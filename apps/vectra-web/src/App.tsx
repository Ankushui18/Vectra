/**
 * VECTRA remote control (Tasks 1.4 + 2.2).
 *
 * DUMB BY DESIGN: this component holds no document, resolves no parameters,
 * computes no geometry, and maintains no dependency graph. It sends
 * `CommandWire` JSON to the WASM engine, appends returned `EngineEvent`s to
 * the log, and re-renders the latest `SnapshotWire` plus the graph the engine
 * exports (`dependencies()`). The dependency panel is a pure view projection of
 * engine data — engine truth stays in Rust.
 *
 * Task 5.0 replaced the SVG projection with a WebGPU canvas. That removal is the
 * point: the old preview recomputed every shape's SVG geometry in TypeScript,
 * which meant two implementations of the document (one in Rust, one in the DOM).
 * Now the engine's `EvaluatedScene` goes straight to the GPU, and the only thing
 * this file knows about a shape is the `NodeId` the renderer hands back.
 *
 * What Task 2.2 added here is *visibility*: the `Dirty` event's ids highlight
 * the exact layers that were re-evaluated, the `eval` block shows that the
 * document was evaluated once and patched ever after, and "↻ Full re-eval"
 * proves the incremental result equals a rebuild.
 *
 * Task 10.1 added the **drawing suite** to that remote, and the way it was added
 * is the point: a pen, a brush and a white arrow whose every gesture becomes one
 * engine call (`draw_pointer`, the two commits, `draw_quick_shape`, the
 * `draw_edit_*` triad). The interaction *policy* lives in `engine/draw/*` as pure
 * functions (unit-tested in `tests/draw.test.ts`), the geometry lives in Rust, and
 * this file — the one that routes pointer events — only decides which button the
 * user pressed. RULE 4's "the math stays hidden" is a single `showsMathPanels()`
 * call below: while a drawing tool is held, the parametric panels are not
 * rendered at all.
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import type { PointerEvent as ReactPointerEvent, ReactNode, RefObject } from 'react';
import { VectraClient } from './engine/client';
import { documentHost } from './engine/host';
import { DocumentBar } from './components/DocumentBar';
import type { FileLogKind } from './components/DocumentBar';
import ToolPalette, { DesignerRail } from './components/ToolPalette';
import DrawOverlay from './components/DrawOverlay';
import NavigationOverlay from './components/NavigationOverlay';
import LayersPanel from './components/LayersPanel';
import AppearancePanel from './components/AppearancePanel';
import ArtboardBar from './components/ArtboardBar';
import ComponentPanel from './components/ComponentPanel';
import MagicBar from './components/MagicBar';
import DrawSettingsPanel from './components/DrawSettingsPanel';
import { designerHint, showsMathPanels, toolForShortcut } from './engine/draw/tools';
import type { ToolId } from './engine/draw/tools';
import { DrawSession, initialSession, pointerCursor } from './engine/draw/session';
import type { DrawSessionState } from './engine/draw/session';
import {
  cancelIntent,
  downIntent,
  DRAG_SLOP_PX,
  finishIntent,
  isDrag,
  moveIntent,
  strokePoints,
  upIntent,
} from './engine/draw/pointer';
// **Task 10.7**: the invisible UI (RULE 1), the smooth hand (RULE 2) and the
// drop (RULE 4). Three pure modules, and this file is the only place that turns
// their answers into engine calls — the recognisers decide, `App` dispatches.
import { modifierClick, TouchRouter } from './engine/draw/touch';
import type { TouchAction } from './engine/draw/touch';
import { StreamLine, streamlineAmount } from './engine/draw/streamline';
import {
  dropChangesStack,
  dropStatus,
  planColorDrop,
  stackWithColor,
} from './engine/draw/colordrop';
import { holdHint } from './engine/draw/quick-shape';
import {
  anchorLabel,
  editTarget,
  overlayModel,
  parseHit,
  parseOverlay,
  placeOverlay,
  revealedHandles,
} from './engine/draw/path-edit';
import type { PlacedOverlay } from './engine/draw/path-edit';
import {
  addConstraint,
  addProceduralNode,
  assignNodeToLayer,
  batch,
  beginDrag,
  bindTrack,
  booleanOperation,
  coincidentConstraints,
  connectProcedural,
  createBoundCircle,
  createBoundRectangle,
  createCircle,
  createGroup,
  createRectangle,
  defineExpression,
  deleteNode,
  disconnectProcedural,
  distanceConstraint,
  duplicateNode,
  endDrag,
  modifierOperation,
  setAppearances,
  motionTrack,
  pointOperand,
  proceduralNode,
  removeConstraint,
  removeExpression,
  removeOperation,
  removeProceduralNode,
  scalarOperand,
  setConstraintEnabled,
  setOperationEnabled,
  setNodeParent,
  setProceduralEnabled,
  setProceduralOperand,
  setVariable,
  unbindParam,
  updateDrag,
  verticalConstraint,
} from './engine/commands';
import type { ConstrainedLayer } from './engine/commands';
import {
  aiPanelState,
  constraintRows,
  dependencyRows,
  dirtyIdsOf,
  dragStatus,
  evalSummary,
  formatEvent,
  layerRows,
  motionRows,
  motionSummary,
  exportSummary,
  operationRows,
  proceduralPortOptions,
  proceduralRows,
  proceduralSummary,
  solverSummary,
  stateChips,
  summaryCounts,
  summaryLabels,
} from './engine/view-model';
import {
  appearanceStack,
  gridStyle,
  overlayBoards,
  panGestureAllowed,
  showsAppearancePanel,
  selectedNode,
  stackToWire,
  wheelZoomFactor,
  ZOOM_STEP,
} from './engine/panels';
import type { LogLine } from './engine/view-model';
import type {
  AiCallWire,
  AiExecuteWire,
  DrawReplyWire,
  BooleanOpWire,
  CanvasStatusWire,
  CanvasViewWire,
  CommandWire,
  ExportEnvelopeWire,
  FrameWire,
  DependencyResponseWire,
  MotionWire,
  ProceduralKindOptionWire,
  DocumentSummaryWire,
  ProceduralReportWire,
  SnapshotWire,
  StrengthWire,
  ComponentPropWire,
  ComponentViewWire,
  StructuralMacroWire,
} from './engine/wire';

// ── Log ──────────────────────────────────────────────────────────────────

interface LogEntry extends LogLine {
  seq: number;
}

// ── Canvas (WebGPU, Task 5.0) ────────────────────────────────────────────

/**
 * The canvas pane. It renders nothing itself and decides nothing itself: the
 * engine's scene goes to the GPU through `VectraClient.renderFrame`, and a
 * pointer goes *in* as client coordinates and comes back as a `NodeId`.
 *
 * The `<canvas>` element is the only thing React owns here. Every pixel on it
 * was drawn by `wgpu` from `EvaluatedScene` data, and every click that lands on
 * a shape was resolved by the renderer's spatial index — not by the DOM.
 */
interface CanvasPaneProps {
  canvasRef: RefObject<HTMLCanvasElement>;
  onPointerDown: (e: ReactPointerEvent<HTMLCanvasElement>) => void;
  /** Task 10.3 RULE 2: a pan drag (middle button, or space + drag). Capture
   *  phase, so a pan can take the gesture before the active tool sees it. */
  onPanDown: (e: ReactPointerEvent<HTMLDivElement>) => void;
  onPanMove: (e: ReactPointerEvent<HTMLDivElement>) => void;
  onPanUp: (e: ReactPointerEvent<HTMLDivElement>) => void;
  /** The document grid, sized and offset by the camera. */
  grid: { backgroundSize: string; backgroundPosition: string };
  /** Task 6.0: report the pointer's position so the engine can hover it. */
  onPointerMove: (e: ReactPointerEvent<HTMLCanvasElement>) => void;
  /** …and report that it left, which is not a position. */
  onPointerLeave: () => void;
  /** **RULE 1**: the touch gesture's decision point — a two- or three-finger
   *  tap is decided when the last contact lifts, not when it lands. */
  onPointerUp: (e: ReactPointerEvent<HTMLCanvasElement>) => void;
  /** A contact was lost (a browser takeover, a blur): the gesture ends, and
   *  nothing fires. A cancelled gesture must never undo. */
  onPointerCancel: (e: ReactPointerEvent<HTMLCanvasElement>) => void;
  /** True once the engine has at least one node to draw. */
  hasNodes: boolean;
  /** GPU status line; `null` while the device is still coming up. */
  status: CanvasStatusWire | null;
  /** The last frame's cost, for the incrementality readout. */
  frame: FrameWire | null;
  /** The cursor for the active tool (Task 10.1 RULE 4: "show the tool cursor"). */
  cursor: string;
  /** The drawing overlay — anchors, handles, the in-progress path. */
  overlay?: ReactNode;
}

function CanvasPane({
  canvasRef,
  onPointerDown,
  onPanDown,
  onPanMove,
  onPanUp,
  onPointerMove,
  onPointerLeave,
  onPointerUp,
  onPointerCancel,
  hasNodes,
  status,
  frame,
  cursor,
  overlay,
  grid,
}: CanvasPaneProps) {
  const down = status?.ready === false;
  return (
    <div
      className="canvas-wrap"
      // **RULE 1, the browser's half.** A canvas is a context-menu target on
      // every platform, and a long press on a touchscreen is how a designer
      // opens one by accident. The menu is refused at the *pane*, so the canvas
      // and every overlay inside it inherit the refusal — the gesture layer in
      // `engine/draw/touch.ts` can then own the multi-finger gestures without
      // the browser answering first.
      onContextMenu={(event) => event.preventDefault()}
      onPointerDownCapture={onPanDown}
      onPointerMoveCapture={onPanMove}
      onPointerUpCapture={onPanUp}
      onPointerCancelCapture={onPanUp}
    >
      <canvas
        ref={canvasRef}
        className="preview"
        data-testid="preview"
        width={800}
        height={600}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerLeave={onPointerLeave}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerCancel}
        style={{ touchAction: 'none', cursor }}
      />
      {/* Task 10.1 RULE 4: the overlay shows the cursor, the path being drawn
          and the Bézier handles — every coordinate of it placed by the engine's
          own camera (`client.documentToClient`), never by a transform here. */}
      {overlay}
      {/* The doc grid is a CSS overlay, but it is *the camera's* grid: its cell
          is 50 document units and its origin is the visible window's top-left,
          both taken from the renderer's own view. A grid that stayed put while
          the artwork panned would be a wrong ruler, which is worse than none. */}
      <div className="canvas-grid" style={grid} aria-hidden="true" />
      {!hasNodes && (
        <div className="canvas-hint" data-testid="preview-hint">
          Empty scene — Add a circle to begin the loop
        </div>
      )}
      {down && (
        <div className="canvas-banner" data-testid="canvas-banner">
          WebGPU unavailable — the scene is drawn by the engine but there is no
          GPU to present it.
          {status?.error ? <span className="sub"> {status.error}</span> : null}
        </div>
      )}
      <div className="canvas-frame" data-testid="frame-readout">
        {frame
          ? `frame ${frame.frame} · dirty ${frame.dirty}${frame.full ? ' (full)' : ''} · wrote ${
              frame.writes
            } buffer${frame.writes === 1 ? '' : 's'} ${frame.bytes} B${
              frame.ramps > 0 ? ` + ${frame.ramps} B ramps` : ''
            } · draw ${frame.draw_calls}`
          : 'frame —'}
      </div>
    </div>
  );
}

// ── App ──────────────────────────────────────────────────────────────────

type Status = 'loading' | 'ready' | 'error';

/**
 * **The gesture's name, in the designer's words** (Task 10.7 RULE 1) — what the
 * event log says happened. Kept at module scope because it is a *vocabulary*
 * table, not state: the recogniser speaks in `TouchAction`s, the log speaks in
 * taps and swipes, and this is the only place the two meet.
 */
const GESTURE_LABEL: Record<TouchAction, string> = {
  undo: 'two-finger tap',
  redo: 'three-finger tap',
  copyPaste: 'three-finger swipe down',
  none: '',
};

/**
 * Task 10.0 note: this component takes no props. Both builds run *this* tree,
 * and in both of them the engine is the one in this page (the web build's wasm
 * module; the desktop window's identical wasm module). What differs between the
 * builds is only where a **file** lives, and that is the `DocumentHost` seam:
 * `documentHost()` below is the browser's refusal on the web and the Tauri
 * host — native dialogs, `.vectra` codec — inside the desktop window.
 *
 * The reason there is no `runner` prop is a finding, not an oversight: the
 * engine is `!Send` (cassowary's rows are `Rc<RefCell<..>>`), so it cannot live
 * in Tauri's managed state and there is no second engine to keep in step. One
 * engine, one document, one history — in both builds.
 */
export default function App() {
  const [status, setStatus] = useState<Status>('loading');
  const [statusDetail, setStatusDetail] = useState('Loading WASM engine…');
  const [client, setClient] = useState<VectraClient | null>(null);
  /** The document host this build installed (desktop) or the browser's refusal. */
  const host = documentHost();
  const [snapshot, setSnapshot] = useState<SnapshotWire | null>(null);
  const [log, setLog] = useState<LogEntry[]>([]);
  const [deps, setDeps] = useState<DependencyResponseWire | null>(null);
  const [lastDirty, setLastDirty] = useState<string[]>([]);
  const [varName, setVarName] = useState('base');
  const [varValue, setVarValue] = useState('200');
  const [exprSource, setExprSource] = useState('$base * 2 + 10');
  /** Task 4.0: the operand a modifier action sends (`offset 4`, `fillet 4`,
   *  `mirror across x = 4`). One number, forwarded — never resolved here. */
  const [modValue, setModValue] = useState('4');
  /** Selected layer ids, in click order. Two of them enable the constraint
   *  actions — the constraint builders address the FIRST as the anchor. */
  const [selection, setSelection] = useState<string[]>([]);
  /** Which layer rows the Layers Panel has opened (RULE 1's "collapsible"). */
  const [expandedLayers, setExpandedLayers] = useState<ReadonlySet<string>>(new Set());
  /** Task 3.2: the live gesture. `dx`/`dy` are the pointer's grab offset in
   *  document units — the ONLY arithmetic the UI performs (a view offset, not
   *  geometry): every sample it sends is `pointer − offset`. */
  const [drag, setDrag] = useState<{ id: string; dx: number; dy: number } | null>(null);
  /** Task 5.0: the GPU's status, and the cost of the last drawn frame. */
  const [canvas, setCanvas] = useState<CanvasStatusWire | null>(null);
  const [frame, setFrame] = useState<FrameWire | null>(null);
  /** Task 10.3 RULE 2: the document rectangle on screen, and the zoom. Both are
   *  *reads of the renderer* — refreshed after every gesture, jump and resize —
   *  never a second opinion about where the camera is. */
  const [view, setView] = useState<CanvasViewWire | null>(null);
  const [zoom, setZoom] = useState<number | null>(null);
  /** Space held: turns the primary button into a pan grip. */
  const spaceRef = useRef(false);
  /** The live pan drag's anchor, in client coordinates. */
  const panRef = useRef<{ x: number; y: number } | null>(null);
  /** Task 6.0: the engine's motion state (bindings, horizon, state flags). */
  const [motion, setMotion] = useState<MotionWire | null>(null);
  /** Task 6.0: the idle signal — `false` means the frame loop is stopped. */
  const [animating, setAnimating] = useState(false);
  /** Task 7.0: the engine's palette (kinds + operand payloads) and the chain's
   *  own report. The UI keeps no table of kinds, ports or defaults. */
  const [palette, setPalette] = useState<ProceduralKindOptionWire[]>([]);
  const [procedural, setProcedural] = useState<ProceduralReportWire | null>(null);
  /** The kind the "Add node" control will create next, and the layer a `source`
   *  node will read. */
  const [procKind, setProcKind] = useState('');
  const [procSubject, setProcSubject] = useState('');
  /** Numbers being typed into an operand's Set control, keyed `node:port`. */
  const [procOperands, setProcOperands] = useState<Record<string, string>>({});
  /** The source port chosen in a connect control, keyed `node:port`. */
  const [procWiring, setProcWiring] = useState<Record<string, string>>({});
  /** Task 8.0: which exporter the Export section will run, and the last answer.
   *  The modal is "there is a result", not a second piece of state to keep in
   *  sync with it. */
  const [exportFormat, setExportFormat] = useState('svg');
  const [exportResult, setExportResult] = useState<ExportEnvelopeWire | null>(null);
  /** Task 9.0: the AI panel. `aiPreview` is the plan on the table, `aiReceipt`
   *  is what the engine said after running it. Two envelopes, no third state:
   *  the panel folds them for display and never keeps a plan of its own. */
  const [aiText, setAiText] = useState('round the corners of card by 8');
  const [aiPreview, setAiPreview] = useState<AiCallWire | null>(null);
  const [aiReceipt, setAiReceipt] = useState<AiExecuteWire | null>(null);
  const [aiShowPrompt, setAiShowPrompt] = useState(false);
  const [aiPromptText, setAiPromptText] = useState('');
  const [aiPhrasings, setAiPhrasings] = useState<string[]>([]);
  /** The engine's own summary — the grounding the panel *shows* (RULE 2). */
  const [aiSummary, setAiSummary] = useState<DocumentSummaryWire | null>(null);

  // ── Task 10.6: Make Magic (Cmd+K) and Smart Components ────────────────────
  /** The Cmd+K bar. `magicOpen` is the palette; `magicText` is the prompt. */
  const [magicOpen, setMagicOpen] = useState(false);
  const [magicText, setMagicText] = useState('');
  /** The engine's own prose for the current selection — what a prompt acts on. */
  const [selectionProse, setSelectionProse] = useState('');
  /** The structural macros, as chips under the bar. */
  const [macros, setMacros] = useState<StructuralMacroWire[]>([]);
  /** The Smart Component inspector's state (props, role, headline). */
  const [component, setComponent] = useState<ComponentViewWire | null>(null);
  /** The designer's sentence from the last Magic run (RULE 4). */
  const [magicReceipt, setMagicReceipt] = useState<string | null>(null);
  /** Icon Studio's size list, as a text field: "16 32 48". */
  const [iconSizes, setIconSizes] = useState('16 32 48');
  /** The two numbers a hover spring interpolates between (document units). */
  const [hoverOff, setHoverOff] = useState('200');
  const [hoverOn, setHoverOn] = useState('320');

  // ── Task 10.1: the drawing suite ──────────────────────────────────────
  //
  // The session is a *ref*, not state: it is the mutable mirror of what the
  // engine has been told, and `setDraw` is only how a render is triggered. The
  // alternative (the session in state, mutations through a reducer) would make
  // every pointer sample depend on React's batching being prompt, and a dropped
  // sample in a brush stroke is a visible flat spot in the curve.
  const sessionRef = useRef<DrawSession>(new DrawSession());
  const [draw, setDraw] = useState<DrawSessionState>(() => initialSession());
  const holdTimer = useRef<number | null>(null);
  const [placed, setPlaced] = useState<PlacedOverlay>({ points: [], lines: [] });
  const canvasRef = useRef<HTMLCanvasElement>(null);
  /** The canvas attaches once per page, not once per React render. */
  const attached = useRef(false);
  /** The pending animation-frame request, `0` when the loop is stopped. */
  const rafRef = useRef(0);
  const seq = useRef(0);
  const logRef = useRef<HTMLDivElement>(null);
  /** Where ⌘K/Ctrl+K puts the caret. */
  const aiPromptRef = useRef<HTMLInputElement>(null);

  // ── Task 10.7: the "Procreate" layer ──────────────────────────────────
  //
  // Three pieces of state, each owned by the gesture that needs it:
  // * `streamline` is the slider's number and nothing else — the filter is built
  //   per stroke from it, so moving the slider mid-stroke cannot change the
  //   shape of the stroke already under the hand.
  // * `dropColor`/`dropPos` are the ColorDrop drag: the colour the well carries
  //   and where the pointer is while it is carried. The drop itself is decided
  //   on release, against the engine's hit test.
  const [streamline, setStreamline] = useState(30);
  const [dropColor, setDropColor] = useState('#e8622c');
  const [dropPos, setDropPos] = useState<{ x: number; y: number } | null>(null);
  /** **RULE 1**: the multi-touch recogniser. A ref, because it is the gesture's
   *  state across events, not a value the UI renders. */
  const touchRef = useRef<TouchRouter>(new TouchRouter());
  /** **RULE 2**: the live stroke's filter, `null` between strokes. */
  const streamlineRef = useRef<StreamLine | null>(null);
  /** The Ctrl-click candidate (RULE 1's mouse half): armed on press, decided
   *  on release — a Ctrl-*drag* is not a click. */
  const modifierClickRef = useRef<{
    action: TouchAction;
    pointerId: number;
    x: number;
    y: number;
  } | null>(null);

  const appendLog = useCallback((kind: LogEntry['kind'], text: string) => {
    seq.current += 1;
    const entry = { seq: seq.current, kind, text };
    setLog((prev) => [...prev.slice(-199), entry]);
  }, []);

  useEffect(() => {
    const el = logRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [log]);

  // A selection that points at a layer the engine no longer has (deleted, or
  // undone away) is dropped — the UI never invents state the engine lacks.
  useEffect(() => {
    if (!snapshot) return;
    setSelection((prev) => prev.filter((id) => id in snapshot.scene.nodes));
  }, [snapshot]);

  // Engine init (once — client module singletons the WASM instantiation).
  useEffect(() => {
    let cancelled = false;
    VectraClient.create()
      .then((c) => {
        if (cancelled) return;
        setClient(c);
        // The palette is engine data, fetched once: it is a constant table.
        const kinds = c.proceduralKinds();
        setPalette(kinds);
        setProcKind((prev) => prev || (kinds[0]?.tag ?? ''));
        setSnapshot(c.snapshot());
        setProcedural(c.procedural());
        setStatus('ready');
        setStatusDetail('Engine ready');
        appendLog(
          'info',
          `engine: WASM instantiated, snapshot loaded, ${kinds.length} procedural kinds in the palette`,
        );
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setStatus('error');
        setStatusDetail(`Engine failed: ${e instanceof Error ? e.message : e}`);
        appendLog('error', `engine init failed: ${e instanceof Error ? e.message : e}`);
      });
    return () => {
      cancelled = true;
    };
  }, [appendLog]);

  /**
   * **The render loop (Task 6.0).** One `render_frame` per displayed frame *while
   * something is moving*, and then it stops.
   *
   * Task 5.0's loop ran forever and simply drew nothing when the scene was at
   * rest — a 60 Hz timer, a React state check and a GPU-sync call per frame, for
   * a picture that could not change. The engine can now answer the question that
   * makes stopping safe (`is_animating`), so the loop asks it after every frame
   * and goes back to sleep. `kick()` is the other half: every mutation wakes it,
   * and a wake-up while it is already running is a no-op.
   *
   * The stopping rule is the engine's, not the UI's: a spring is "moving" until
   * its settle horizon, a track until its last keyframe. The UI never counts
   * frames or watches for duplicate pixels — a heuristic here would be a second,
   * wrong opinion about motion.
   */
  const kick = useCallback(() => {
    if (!client || !canvas?.ready || rafRef.current) return;
    const tick = () => {
      rafRef.current = 0;
      const drawn = client.renderFrame();
      if (drawn) {
        if (drawn.error) appendLog('error', `canvas: ${drawn.error}`);
        else if (drawn.writes > 0 || drawn.removed > 0 || drawn.full) setFrame(drawn);
      }
      if (client.isAnimating()) {
        rafRef.current = requestAnimationFrame(tick);
      } else {
        setAnimating(false);
        setFrame((prev) => prev ?? drawn);
      }
    };
    setAnimating(true);
    rafRef.current = requestAnimationFrame(tick);
  }, [client, canvas?.ready, appendLog]);

  // The loop is torn down on unmount (StrictMode double-invokes the effects).
  useEffect(
    () => () => {
      if (rafRef.current) cancelAnimationFrame(rafRef.current);
      rafRef.current = 0;
    },
    [],
  );

  /** Adopt the view a renderer call just returned, in one place: the rectangle,
   *  the zoom the readout prints, and a frame so the change is on screen.
   *
   *  Every navigation entry point (`nav_*`, `frame_document`, `measure`) returns
   *  the camera it left behind, so the overlay is *told* where it is rather than
   *  computing it — the difference between a view of the camera and a second
   *  camera. */
  const adoptView = useCallback(
    (next: CanvasViewWire | null) => {
      if (!client) return;
      setView(next ?? client.canvasView());
      setZoom(client.navScale());
      kick();
    },
    [client, kick],
  );

  const refresh = useCallback((c: VectraClient) => {
    setSnapshot(c.snapshot());
    setMotion(c.motion());
    setProcedural(c.procedural());
    const graph = c.dependencies();
    setDeps(graph.status === 'ok' ? graph : null);
    // Task 10.3 RULE 2: the overlay reads the renderer, so every refresh
    // re-reads it — a snapshot that arrives after a resize must not leave the
    // artboard frames describing the previous camera.
    const canvasView = c.canvasView();
    if (canvasView) setView(canvasView);
    setZoom(c.navScale());
  }, []);

  // ── Task 5.0: the WebGPU canvas ────────────────────────────────────────
  //
  // 1. React hands the renderer the `<canvas>` element on mount.
  // 2. The renderer is measured (its box, its device pixel ratio) so the camera
  //    and the pointer agree on what a pixel means.
  // 3. A frame is requested per animation frame: the engine pushes its scene and
  //    the ids that changed, the GPU draws. Nothing here reads a shape.
  useEffect(() => {
    const element = canvasRef.current;
    if (!client || !element || attached.current) return;
    attached.current = true;
    let cancelled = false;
    client
      .attachCanvas(element)
      .then((status) => {
        if (cancelled) return;
        setCanvas(status);
        appendLog(
          status.ready ? 'info' : 'error',
          status.ready
            ? `canvas: WebGPU up — ${status.pixels[0]}×${status.pixels[1]} px, ${status.draw_calls} draw calls`
            : `canvas: WebGPU unavailable (${status.error ?? 'no adapter'})`,
        );
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        const message = e instanceof Error ? e.message : String(e);
        setCanvas({ ready: false, frames: 0, pixels: [0, 0], nodes: 0, draw_calls: 0, error: message, gpu: null });
        appendLog('error', `canvas: WebGPU failed — ${message}`);
      });
    return () => {
      cancelled = true;
    };
  }, [client, appendLog]);

  // The element resizes with its panel; the backing store and the camera follow.
  useEffect(() => {
    const element = canvasRef.current;
    if (!client || !element || !canvas?.ready || typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(() => {
      client.measureCanvas(element);
      adoptView(client.canvasView());
    });
    observer.observe(element);
    client.measureCanvas(element);
    adoptView(client.canvasView());
    return () => observer.disconnect();
  }, [client, canvas?.ready, adoptView]);

  /**
   * **The wheel (Task 10.3 RULE 2).**
   *
   * A native listener rather than React's `onWheel`, because React attaches
   * wheel handlers passively: `preventDefault` there is a no-op, and a wheel that
   * cannot be prevented scrolls the page *and* zooms the canvas — the user sees
   * the artwork jump out from under the pointer they were aiming at.
   *
   * The handler is re-installed only when the client changes; it reads the live
   * gesture state through a ref, so a wheel during a drag still works and does
   * not re-subscribe on every render.
   */
  const wheelRef = useRef<(event: WheelEvent) => void>(() => {});
  wheelRef.current = (event: WheelEvent) => {
    if (!client || !canvas?.ready) return;
    const factor = wheelZoomFactor(event.deltaY, event.deltaMode);
    if (factor === 1) return;
    event.preventDefault();
    zoomAt(event.clientX, event.clientY, factor);
  };
  useEffect(() => {
    const element = canvasRef.current;
    if (!element) return;
    const handler = (event: WheelEvent) => wheelRef.current(event);
    element.addEventListener('wheel', handler, { passive: false });
    return () => element.removeEventListener('wheel', handler);
  }, [client]);

  /**
   * **Space is a pan grip** while it is held, and the cursor says so. Tracked
   * here rather than in the tool state because it is not a tool: it *modifies*
   * whichever tool is active, exactly like Alt does in the pen.
   */
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      // Space is a pan grip *on the canvas*. On a focused control it is the
      // control's activation key — swallowing it would break keyboard access to
      // every button in the workspace (an eye toggle, a blend menu), and the
      // canvas button itself is covered by the same check.
      if (
        target &&
        (target.isContentEditable ||
          ['INPUT', 'TEXTAREA', 'SELECT', 'BUTTON', 'A'].includes(target.tagName) ||
          target.getAttribute('role') === 'button' ||
          target.closest?.('button, a[href], [role="button"]') !== null)
      ) {
        spaceRef.current = false;
        return;
      }
      if (event.key === ' ') {
        spaceRef.current = event.type === 'keydown';
        if (event.type === 'keydown') event.preventDefault();
      }
    };
    window.addEventListener('keydown', onKey);
    window.addEventListener('keyup', onKey);
    const clear = () => {
      spaceRef.current = false;
    };
    window.addEventListener('blur', clear);
    return () => {
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('keyup', onKey);
      window.removeEventListener('blur', clear);
    };
  }, []);


  // The first frame after the canvas comes up. A scene that is not moving gets
  // exactly one frame — enough to draw it — and the loop stays stopped.
  useEffect(() => {
    if (!client || !canvas?.ready) return;
    kick();
  }, [client, canvas?.ready, kick]);

  /** Format a response's events into the log and remember the dirty set. */
  const logResponse = useCallback(
    (res: ReturnType<VectraClient['dispatch']>) => {
      if (res.status === 'ok') {
        for (const e of res.events) {
          const line = formatEvent(e);
          appendLog(line.kind, line.text);
        }
        const dirty = dirtyIdsOf(res.events);
        if (dirty !== null) setLastDirty(dirty);
      } else {
        appendLog('error', `✗ ${res.message}`);
      }
    },
    [appendLog],
  );

  /** Consume a response, re-read the snapshot + graph, and wake the loop. */
  const absorb = useCallback(
    (c: VectraClient, res: ReturnType<VectraClient['dispatch']>) => {
      logResponse(res);
      refresh(c);
      kick();
    },
    [logResponse, refresh, kick],
  );

  /** THE LOOP: build command → dispatch → log events → refresh snapshot+graph. */
  const runCommand = useCallback(
    (label: string, cmd: CommandWire) => {
      if (!client) return;
      appendLog('cmd', `→ ${label} ${JSON.stringify(cmd)}`);
      absorb(client, client.dispatch(cmd));
    },
    [client, appendLog, absorb],
  );

  /** **Jump the camera to a document rectangle** (Task 10.2 RULE 2): the
   *  artboard dropdown's whole pan/zoom story. The rectangle comes from the
   *  engine's artboard record; the projection stays in the renderer. */
  const frameDocument = useCallback(
    (rect: { x: number; y: number; width: number; height: number }) => {
      if (!client) return;
      const framed = client.frameDocument(rect);
      if (framed) {
        appendLog('info', `framed ${JSON.stringify(rect)} → ${JSON.stringify(framed)}`);
      }
      adoptView(framed);
    },
    [client, appendLog, adoptView],
  );

  /** Zoom by a factor about a client-space point — the wheel's own entry. */
  const zoomAt = useCallback(
    (clientX: number, clientY: number, factor: number) => {
      if (!client) return;
      adoptView(client.navZoom(clientX, clientY, factor));
    },
    [client, adoptView],
  );

  /** Zoom by a factor about the middle of the canvas — the `+`/`−` buttons and
   *  the keyboard, where there is no pointer to anchor to. */
  const zoomBy = useCallback(
    (factor: number) => {
      const element = canvasRef.current;
      if (!client || !element) return;
      const box = element.getBoundingClientRect();
      zoomAt(box.left + box.width / 2, box.top + box.height / 2, factor);
    },
    [client, zoomAt],
  );

  /** An absolute zoom, in CSS pixels per unit: `1` is the "100%" button. */
  const zoomTo = useCallback(
    (scale: number) => {
      if (!client) return;
      adoptView(client.navZoomTo(scale));
    },
    [client, adoptView],
  );

  /** Fit the whole document — the camera's opening view, on demand. */
  const zoomFit = useCallback(() => {
    if (!client) return;
    adoptView(client.frameDocumentDefault());
  }, [client, adoptView]);

  /** **Pan and zoom** (Task 10.3 RULE 2): a middle-button drag, or space + drag.
   *  The gesture is one camera transition per sample, applied to the camera on
   *  screen right now — the renderer's, not a UI-side transform. */
  const onPanDown = useCallback(
    (e: ReactPointerEvent<HTMLDivElement>) => {
      if (!client || !panGestureAllowed(e.button, spaceRef.current)) return;
      e.preventDefault();
      // Capture: a pan is not the active tool's gesture, and the tool's own
      // pointerdown handler must not also see it.
      e.stopPropagation();
      panRef.current = { x: e.clientX, y: e.clientY };
      e.currentTarget.setPointerCapture?.(e.pointerId);
    },
    [client],
  );

  const onPanMove = useCallback(
    (e: ReactPointerEvent<HTMLDivElement>) => {
      const anchor = panRef.current;
      if (!client || !anchor) return;
      e.stopPropagation();
      const view = client.navPan(e.clientX - anchor.x, e.clientY - anchor.y);
      panRef.current = { x: e.clientX, y: e.clientY };
      adoptView(view);
    },
    [client, adoptView],
  );

  const onPanUp = useCallback((e: ReactPointerEvent<HTMLDivElement>) => {
    if (!panRef.current) return;
    panRef.current = null;
    e.currentTarget.releasePointerCapture?.(e.pointerId);
  }, []);

  /**
   * **Group the selection** (Task 10.4 RULE 1) — one user action, one undo entry,
   * and every step of it an ordinary command: a `Batch` creates the group node
   * and moves each selected node inside, in the selection's own order.
   *
   * There is no "group" verb in the engine and no group concept in the document
   * beyond `NodeKind::Group` — which is why nothing else had to change for the
   * panel to draw a group, the canvas to move its children, or the exporter to
   * write it. The group lands in the layer of the first selected node, the layer
   * the designer was working in.
   */
  const groupSelection = useCallback(() => {
    if (!client || selection.length === 0) return;
    const group = crypto.randomUUID();
    const name = selection.length === 1 ? 'Group' : `Group ${selection.length}`;
    absorb(
      client,
      client.dispatch(
        batch([
          createGroup(group, name),
          ...selection.map((nodeId, index) => setNodeParent(nodeId, group, index)),
        ]),
      ),
    );
  }, [client, selection, absorb]);

  /** **Export current artboard** (RULE 2): the engine's own exporter, showing
   *  the file in the same panel the other exports use. */
  const exportCurrentArtboard = useCallback(() => {
    if (!client) return;
    const result = client.exportCurrentArtboard();
    setExportResult(result);
    appendLog('ok', `export current artboard: ${result.code.length} chars of SVG`);
  }, [client, appendLog]);

  /** **Export all artboards** (RULE 2) — one file, every board where it sits. */
  const exportAllArtboards = useCallback(() => {
    if (!client) return;
    const result = client.exportAllArtboards();
    setExportResult(result);
    appendLog('ok', `export all artboards: ${result.code.length} chars of SVG`);
  }, [client, appendLog]);

  /** Several commands as one user action (the engine still sees them one by one,
   *  gated, recorded, and reported separately — the UI just batches the UX). */
  const runSequence = useCallback(
    (label: string, cmds: CommandWire[]) => {
      if (!client) return;
      appendLog('cmd', `→ ${label}`);
      for (const cmd of cmds) {
        appendLog('cmd', `  ↗ ${JSON.stringify(cmd)}`);
        const res = client.dispatch(cmd);
        logResponse(res);
        if (res.status === 'error') break;
      }
      refresh(client);
      kick();
    },
    [client, appendLog, logResponse, refresh, kick],
  );

  const runUndo = useCallback(() => {
    if (!client) return;
    appendLog('cmd', '→ undo');
    absorb(client, client.undo());
  }, [client, appendLog, absorb]);

  const runRedo = useCallback(() => {
    if (!client) return;
    appendLog('cmd', '→ redo');
    absorb(client, client.redo());
  }, [client, appendLog, absorb]);

  /**
   * **Copy / Paste** (Task 10.7 RULE 1): the three-finger swipe's command.
   *
   * The engine has no clipboard command, and it does not need one: its copy
   * primitive is `DuplicateNode`, and a paste is a duplicate placed just above
   * its source. So the gesture duplicates the selection — the designer's
   * "copy/paste", done in one step, which is also what Procreate's Copy & Paste
   * HUD produces on the canvas. The copies become the selection, so a second
   * swipe duplicates the copies (the ordinary "paste again").
   *
   * With nothing selected the gesture does nothing and says so: an invisible
   * failure is worse than no feature.
   */
  const runCopyPaste = useCallback(() => {
    if (!client) return;
    const ids = selection;
    if (ids.length === 0) {
      appendLog('info', '⇩ copy/paste · nothing is selected');
      return;
    }
    const copies: string[] = [];
    for (const id of ids) {
      const command = duplicateNode(id);
      const res = client.dispatch(command);
      absorb(client, res);
      if (res.status === 'error') {
        appendLog('error', `✗ copy/paste · ${res.message}`);
        return;
      }
      const newId = (command as { id?: string }).id;
      if (newId) copies.push(newId);
    }
    if (copies.length > 0) {
      setSelection(copies);
      // The engine's idea of "this" follows the canvas selection, so a prompt
      // typed after the swipe acts on the copies — the same sync the click path
      // does, for the same reason.
      const prose = client.setSelection(copies)?.prose;
      if (prose) setSelectionProse(prose);
    }
    appendLog(
      'cmd',
      `⇩ copy/paste · duplicated ${copies.length} object${copies.length === 1 ? '' : 's'}`,
    );
  }, [client, selection, absorb, appendLog]);

  /** **RULE 1's routing table**, in one place: what a recognised gesture means. */
  const runTouchAction = useCallback(
    (action: TouchAction) => {
      if (action === 'undo') runUndo();
      if (action === 'redo') runRedo();
      if (action === 'copyPaste') runCopyPaste();
    },
    [runUndo, runRedo, runCopyPaste],
  );

  /**
   * **The canvas's release** (RULE 1): where a tap and a modifier click are
   * decided. Left and right gestures: a touch contact's decision is the
   * recogniser's (`touch.ts`), a mouse click's is the modifier's
   * (`modifierClick`), and neither needs to know about the other.
   *
   * The drawing gesture's own release is *not* here: while a stroke is live the
   * window owns the pointer (see `drawGestureUp`), exactly as before.
   */
  const onCanvasUp = useCallback(
    (e: ReactPointerEvent<HTMLCanvasElement>) => {
      if (e.pointerType === 'touch') {
        const decision = touchRef.current.up(
          { pointerId: e.pointerId, clientX: e.clientX, clientY: e.clientY },
          performance.now(),
        );
        if (decision.swallow) e.preventDefault();
        if (decision.action !== 'none') {
          appendLog('info', `⇱ ${GESTURE_LABEL[decision.action]} → ${decision.action}`);
          runTouchAction(decision.action);
        }
        return;
      }
      const candidate = modifierClickRef.current;
      if (!candidate || candidate.pointerId !== e.pointerId) return;
      modifierClickRef.current = null;
      const dragged =
        Math.hypot(e.clientX - candidate.x, e.clientY - candidate.y) >= DRAG_SLOP_PX;
      const action = modifierClick({
        ctrlKey: e.ctrlKey,
        shiftKey: e.shiftKey,
        button: e.button,
        dragged,
      });
      if (action === 'none') return;
      e.preventDefault();
      if (action !== candidate.action) {
        // The modifier set changed between press and release (Shift added, Ctrl
        // let go): the *release* is what the designer meant, so re-read it and
        // say which one ran.
        appendLog('info', `⌨ ctrl-click → ${action}`);
      }
      runTouchAction(action);
    },
    [appendLog, runTouchAction],
  );

  /** A contact was lost: the recogniser forgets it and fires nothing. */
  const onCanvasCancel = useCallback((e: ReactPointerEvent<HTMLCanvasElement>) => {
    if (e.pointerType === 'touch') {
      touchRef.current.cancel({ pointerId: e.pointerId, clientX: e.clientX, clientY: e.clientY });
    }
    modifierClickRef.current = null;
  }, []);

  // ── Task 10.7 RULE 4: ColorDrop's drag ────────────────────────────────
  //
  // The well starts the drag; the *window* finishes it, for the same reason a
  // stroke lives on the window: a drop that leaves the canvas column and comes
  // back must still be the same drag.

  const startColorDrop = useCallback((clientX: number, clientY: number) => {
    setDropPos({ x: clientX, y: clientY });
    appendLog('info', '◧ ColorDrop · drag onto a shape');
  }, [appendLog]);

  const finishColorDrop = useCallback(
    (clientX: number, clientY: number) => {
      setDropPos(null);
      if (!client) return;
      const rect = canvasRef.current?.getBoundingClientRect();
      if (!rect || clientX < rect.left || clientX > rect.right || clientY < rect.top || clientY > rect.bottom) {
        appendLog('info', '◧ ColorDrop · released outside the canvas — nothing to fill');
        return;
      }
      // **The rule's own tool**: the renderer's hit test, the same index a click
      // uses. No UI-side region maths exists to go wrong.
      const nodeId = client.pointerHit(clientX, clientY);
      const plan = planColorDrop(nodeId, dropColor);
      if (plan.kind === 'noop') {
        appendLog('info', dropStatus(plan, null));
        return;
      }
      const node = snapshot?.scene.nodes[plan.nodeId] ?? null;
      const stack = appearanceStack(node);
      if (!dropChangesStack(stack, plan.color)) {
        appendLog('info', `◧ ColorDrop · ${node?.name ?? plan.nodeId} is already that colour`);
        return;
      }
      runCommand('ColorDrop fill', setAppearances(plan.nodeId, stackToWire(stackWithColor(stack, plan.color))));
      appendLog('cmd', dropStatus(plan, node?.name ?? null));
    },
    [client, dropColor, snapshot, appendLog, runCommand],
  );

  useEffect(() => {
    if (!dropPos) return;
    const move = (event: PointerEvent) => setDropPos({ x: event.clientX, y: event.clientY });
    const up = (event: PointerEvent) => finishColorDrop(event.clientX, event.clientY);
    const cancel = () => setDropPos(null);
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
    window.addEventListener('pointercancel', cancel);
    return () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      window.removeEventListener('pointercancel', cancel);
    };
  }, [dropPos !== null, finishColorDrop]);

  const runFullReeval = useCallback(() => {
    if (!client) return;
    appendLog('cmd', '→ force full re-evaluation');
    absorb(client, client.forceFullEvaluation());
  }, [client, appendLog, absorb]);

  const runSetVariable = useCallback(() => {
    if (!client) return;
    const name = varName.trim();
    const value = Number(varValue);
    if (!name || !Number.isFinite(value)) {
      appendLog('error', '✗ variable needs a name and a finite number');
      return;
    }
    runCommand(`SetVariable $${name}=${value}`, setVariable(name, value));
  }, [client, varName, varValue, runCommand, appendLog]);

  const runSetTime = useCallback(
    (t: number) => {
      if (!client) return;
      appendLog('info', `time ← ${t.toFixed(1)}`);
      absorb(client, client.setTime(t));
    },
    [client, appendLog, absorb],
  );

  /** File ▸ Open: replay the host's plan into the page engine (Task 10.0). */
  const replayPlan = useCallback(
    (commands: unknown[]) => {
      if (!client) return;
      appendLog('cmd', `→ replay ${commands.length} command(s) into the page engine`);
      for (const command of commands as CommandWire[]) {
        const res = client.dispatch(command);
        logResponse(res);
        if (res.status === 'error') break;
      }
      refresh(client);
      kick();
    },
    [client, appendLog, logResponse, refresh, kick],
  );

  /** The file bar logs with the same kinds the rest of this file uses. */
  const fileLog = useCallback(
    (kind: FileLogKind, text: string) => appendLog(kind, text),
    [appendLog],
  );

  /** Task 2.2 demo: a node that is *driven by* a variable. */
  const runAddBoundRectangle = useCallback(() => {
    if (!client) return;
    const cmds: CommandWire[] = [];
    if (snapshot?.variables.base === undefined) {
      cmds.push(setVariable('base', 200));
    }
    cmds.push(
      createBoundRectangle({
        x: 260,
        y: 260,
        w: 160,
        h: 120,
        variable: 'base',
        name: 'bound-rect',
      }),
    );
    runSequence('Add rect driven by $base', cmds);
  }, [client, snapshot, runSequence]);

  /** Task 2.2 demo: a node driven by an expression (depth 2 in the graph). */
  const runAddBoundCircle = useCallback(() => {
    if (!client) return;
    const cmds: CommandWire[] = [];
    if (snapshot?.variables.base === undefined) {
      cmds.push(setVariable('base', 200));
    }
    const existing = Object.keys(snapshot?.expressions ?? {})[0];
    const exprId = existing ?? crypto.randomUUID();
    if (!existing) {
      cmds.push(defineExpression(exprId, '$base * 2 + 10'));
    }
    cmds.push(
      createBoundCircle({
        cx: 400,
        cy: 420,
        expressionId: exprId,
        name: 'bound-circle',
      }),
    );
    runSequence('Add circle driven by ƒx', cmds);
  }, [client, snapshot, runSequence]);

  const runDefineExpression = useCallback(() => {
    if (!client) return;
    const source = exprSource.trim();
    if (!source) {
      appendLog('error', '✗ expression needs a source (the engine validates it)');
      return;
    }
    runCommand(
      `DefineExpression ${source}`,
      defineExpression(crypto.randomUUID(), source),
    );
  }, [client, exprSource, runCommand, appendLog]);

  /**
   * Toggle a layer in the selection. Two nodes are what a constraint needs, so
   * a third click starts a new pair rather than growing a set.
   */
  const toggleSelection = useCallback((id: string) => {
    setSelection((prev) => {
      if (prev.includes(id)) return prev.filter((other) => other !== id);
      return prev.length >= 2 ? [id] : [...prev, id];
    });
  }, []);

  const selected: ConstrainedLayer[] = selection.flatMap((id): ConstrainedLayer[] => {
    const node = snapshot?.scene.nodes[id];
    return node ? [{ id, primitive: node.primitive.type, name: node.name }] : [];
  });
  const pairReady = selected.length === 2;

  /** Task 3.1: run a constraint command the builders produced (they return
   *  `null` when a layer exposes no float slot on that axis). */
  const runConstraint = useCallback(
    (label: string, cmds: CommandWire[] | CommandWire | null) => {
      if (cmds === null) {
        appendLog('error', `✗ ${label}: a selected layer has no addressable slot`);
        return;
      }
      const list = Array.isArray(cmds) ? cmds : [cmds];
      if (list.length === 1) {
        runCommand(label, list[0]);
      } else {
        runSequence(label, list);
      }
      setSelection([]);
    },
    [appendLog, runCommand, runSequence],
  );

  const runAddVertical = useCallback(() => {
    if (!pairReady) return;
    runConstraint(
      `Add vertical ${selected[0].name} → ${selected[1].name}`,
      verticalConstraint(selected[0], selected[1]),
    );
  }, [pairReady, runConstraint, selected]);

  const runAddCoincident = useCallback(() => {
    if (!pairReady) return;
    runConstraint(
      `Make ${selected[1].name} coincident with ${selected[0].name}`,
      coincidentConstraints(selected[0], selected[1]),
    );
  }, [pairReady, runConstraint, selected]);

  const runAddDistance = useCallback(
    (value?: number, strength?: StrengthWire) => {
      if (!pairReady) return;
      runConstraint(
        value === undefined
          ? `Hold distance ${selected[0].name} ↔ ${selected[1].name}`
          : `Set distance ${value} ${selected[0].name} ↔ ${selected[1].name}`,
        distanceConstraint(selected[0], selected[1], 'x', { value, strength }),
      );
    },
    [pairReady, runConstraint, selected],
  );

  /** Re-assert an existing distance rule at a different value, weakly: the
   *  textbook over-constrained case the engine must resolve by dropping the
   *  weaker rule and telling us. */
  const runOverConstrain = useCallback(() => {
    const existing = Object.values(snapshot?.constraints ?? {}).find(
      (c) => c.kind === 'distance' && c.enabled !== false && c.value != null,
    );
    if (!existing || existing.value == null) {
      appendLog('error', '✗ add a distance rule first (Hold distance), then over-constrain it');
      return;
    }
    runConstraint(
      `Over-constrain: same rule at ${existing.value + 100}, weak`,
      addConstraint({
        kind: 'distance',
        targets: existing.targets.map((t) => ({ nodeId: t.node_id, property: t.property })),
        value: existing.value + 100,
        strength: 'weak',
      }),
    );
  }, [snapshot, runConstraint, appendLog]);

  // ── Task 4.0: non-destructive operations ─────────────────────────────
  //
  // The panel picks a KIND, names the OPERANDS and mints a row id — exactly the
  // three things a dumb remote is allowed to own. The engine keeps the sources
  // (RULE 1), runs `geo` on them (RULE 2) and evaluates the result into the
  // scene as an ordinary node (RULE 3). A refused operation (a stray operand,
  // an arc where a region is required) comes back as a typed error in the log.

  /** A boolean over the two selected sources. `selected[0]` is the left
   *  operand, so `Subtract` removes `selected[1]` from `selected[0]`. */
  const runBoolean = useCallback(
    (op: BooleanOpWire) => {
      if (!pairReady) return;
      runCommand(
        `${op} ${selected[0].name} ${op === 'subtract' ? '−' : '∪'} ${selected[1].name}`,
        booleanOperation(crypto.randomUUID(), op, selected[0].id, selected[1].id),
      );
      setSelection([]);
    },
    [pairReady, runCommand, selected],
  );

  /** A modifier on the single selected source. */
  const runModifier = useCallback(
    (kind: 'offset' | 'fillet' | 'mirror') => {
      const target = selected[0];
      const value = Number(modValue);
      if (!target || !Number.isFinite(value)) {
        appendLog('error', `✗ ${kind}: pick one layer and enter a number`);
        return;
      }
      runCommand(
        `${kind} ${value} ${target.name}`,
        modifierOperation(crypto.randomUUID(), kind, target.id, value),
      );
      setSelection([]);
    },
    [selected, modValue, runCommand, appendLog],
  );

  const runToggleOperation = useCallback(
    (id: string, enabled: boolean, name: string) => {
      runCommand(
        `${enabled ? 'Enable' : 'Park'} ${name}`,
        setOperationEnabled(id, enabled),
      );
    },
    [runCommand],
  );

  const runRemoveOperation = useCallback(
    (id: string, name: string) => {
      runCommand(`Remove ${name}`, removeOperation(id));
    },
    [runCommand],
  );

  // ── Task 7.0: the procedural graph ───────────────────────────────────
  //
  // The panel chooses a KIND from the engine's palette, names a subject for a
  // `source`, wires ports the engine declared, and types operand literals. It
  // never formats a default, never names a port the engine did not name, and
  // never decides whether a wire is legal: a type mismatch or a cycle comes back
  // as a typed rejection, logged and not applied.

  /** The palette entry the Add control is currently pointing at. */
  const procOption = palette.find((k) => k.tag === procKind) ?? null;

  const runAddProceduralNode = useCallback(() => {
    if (!procOption) {
      appendLog('error', '✗ no procedural kind selected');
      return;
    }
    if (procOption.needs_subject && !procSubject) {
      appendLog('error', `✗ ${procOption.label}: pick the layer it should read`);
      return;
    }
    runCommand(
      `Add ${procOption.tag} node`,
      addProceduralNode(proceduralNode(procOption, procSubject || undefined)),
    );
  }, [procOption, procSubject, runCommand, appendLog]);

  // ── Task 8.0: export ────────────────────────────────────────────────
  //
  // One dropdown, one button, one modal. The engine does the work and returns a
  // whole file; the modal shows it, the button copies it, and the warnings the
  // engine attached are shown *above* the code rather than dropped.

  const runExport = useCallback(() => {
    if (!client) return;
    const envelope =
      exportFormat === 'react' ? client.exportToReact() : client.exportToSvg();
    setExportResult(envelope);
    if (envelope.status !== 'ok') {
      appendLog('error', `✗ export: ${envelope.message ?? 'unknown failure'}`);
      return;
    }
    appendLog(
      'ok',
      `exported ${envelope.format} (${envelope.code.length} chars${
        envelope.warnings.length ? `, ${envelope.warnings.length} warning(s)` : ''
      })`,
    );
    for (const warning of envelope.warnings) {
      appendLog('info', `export note: ${warning}`);
    }
  }, [client, exportFormat, appendLog]);

  /** The clipboard is a browser permission, not a guarantee — say which happened. */
  const copyExport = useCallback(async () => {
    const code = exportResult?.code ?? '';
    if (!code) return;
    try {
      await navigator.clipboard.writeText(code);
      appendLog('ok', `copied ${code.length} chars to the clipboard`);
    } catch {
      appendLog('error', '✗ clipboard refused — select the code and copy manually');
    }
  }, [exportResult, appendLog]);

  // ── Task 9.0: the AI Assistant ───────────────────────────────────────
  //
  // The panel is a remote control with one extra column: it shows the JSON the
  // AI produced *before* anything runs. Preview calls `ai_generate_commands`
  // (which applies nothing), Execute hands that exact JSON back to
  // `ai_execute_commands`, and Auto-correct hands the prompt to
  // `ai_execute_with_retry`, which validates through dispatch, feeds a refusal
  // back to the planner and tries again. All three return the engine's events,
  // which go into the same log every other command writes to.

  const ai = aiPanelState(aiPreview, aiReceipt);

  /** Re-read the summary the AI was grounded on — the panel's context line. */
  const loadAiSummary = useCallback(() => {
    if (!client) return;
    try {
      setAiSummary(JSON.parse(client.documentSummary()) as DocumentSummaryWire);
    } catch {
      setAiSummary(null);
    }
  }, [client]);

  const aiGenerate = useCallback(() => {
    if (!client) return;
    const prompt = aiText.trim();
    if (!prompt) {
      appendLog('error', '✗ the AI panel needs a prompt — try “round the corners of card by 8”');
      return;
    }
    appendLog('cmd', `→ ai.generate ${JSON.stringify(prompt)}`);
    const envelope = client.aiGenerateCommands(prompt);
    setAiPreview(envelope);
    setAiReceipt(null);
    if (envelope.status === 'error') {
      appendLog('error', `✗ ai: ${envelope.code} — ${envelope.message}`);
      return;
    }
    // The commands are logged as commands: the AI speaks the same language the
    // buttons do, and the log proves it.
    for (const command of envelope.plan) appendLog('cmd', `  ◇ ${JSON.stringify(command)}`);
    appendLog(
      'ok',
      `ai: ${envelope.plan.length} command(s) planned — nothing applied yet`,
    );
    for (const note of envelope.notes) appendLog('info', `ai note: ${note}`);
  }, [client, aiText, appendLog]);

  const aiExecute = useCallback(
    (withRetry: boolean) => {
      if (!client) return;
      const prompt = aiText.trim();
      if (!prompt) {
        appendLog('error', '✗ the AI panel needs a prompt');
        return;
      }
      appendLog(
        'cmd',
        withRetry ? `→ ai.execute-with-retry ${JSON.stringify(prompt)}` : '→ ai.execute (previewed plan)',
      );
      const envelope = withRetry
        ? client.aiExecuteWithRetry(prompt)
        : client.aiExecuteCommands(prompt, JSON.stringify(aiPreview?.plan ?? []));
      setAiReceipt(envelope);
      if (envelope.status === 'error') {
        appendLog('error', `✗ ai: ${envelope.code} — ${envelope.message}`);
        for (const correction of envelope.corrections) {
          appendLog(
            'info',
            `ai correction ${correction.attempt}: ${correction.error}${
              correction.note ? ` → ${correction.note}` : ''
            }`,
          );
        }
        // A refused plan rolls itself back, so the log says what the document
        // still is rather than leaving the reader guessing.
        appendLog('info', 'the document is unchanged — a failed plan is rolled back');
        refresh(client);
        kick();
        return;
      }
      for (const event of envelope.report.events) {
        const line = formatEvent(event);
        appendLog(line.kind, line.text);
      }
      const dirty = dirtyIdsOf(envelope.report.events);
      if (dirty !== null) setLastDirty(dirty);
      appendLog('ok', `ai: ${envelope.headline}`);
      for (const correction of envelope.report.corrections) {
        appendLog(
          'info',
          `ai self-corrected after attempt ${correction.attempt}: ${correction.error}${
            correction.note ? ` → ${correction.note}` : ''
          }`,
        );
      }
      refresh(client);
      kick();
    },
    [client, aiText, aiPreview, appendLog, refresh, kick],
  );

  const toggleAiPrompt = useCallback(() => {
    if (!client) return;
    setAiShowPrompt((prev) => {
      // The prompt is fetched, never assembled: it is the engine's own text with
      // the engine's own summary in it (RULE 2).
      if (!prev) setAiPromptText(client.aiPrompt());
      return !prev;
    });
  }, [client]);

  // The hint list is engine data: the planner's own phrasings, so the panel can
  // never advertise something the planner cannot do.
  useEffect(() => {
    if (!client) return;
    setAiPhrasings(client.aiPhrasings());
  }, [client]);

  // The context line follows the document like every other read-only panel.
  useEffect(() => {
    if (client && snapshot) loadAiSummary();
  }, [client, snapshot, loadAiSummary]);

  // ⌘K / Ctrl+K focuses the prompt — the "Cmd+K" the task asks for. Nothing is
  // hijacked while a text field has the caret; the shortcut is a jump, not a mode.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.key.toLowerCase() !== 'k') return;
      event.preventDefault();
      const input = aiPromptRef.current;
      if (!input) return;
      input.focus();
      input.select();
      input.scrollIntoView({ block: 'center', behavior: 'smooth' });
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, []);

  const runConnectProcedural = useCallback(
    (nodeId: string, port: string, from: string, label: string) => {
      const [sourceId, sourcePort] = from.split(':');
      if (!sourceId || !sourcePort) {
        appendLog('error', `✗ ${label}: pick a source port`);
        return;
      }
      runCommand(
        `Connect ${label}.${port} ← ${label}:${sourcePort}`,
        connectProcedural(nodeId, port, { node: sourceId, port: sourcePort }),
      );
      setProcWiring((prev) => ({ ...prev, [`${nodeId}:${port}`]: '' }));
    },
    [runCommand, appendLog],
  );

  const runDisconnectProcedural = useCallback(
    (nodeId: string, port: string, label: string) => {
      runCommand(`Disconnect ${label}.${port}`, disconnectProcedural(nodeId, port));
    },
    [runCommand],
  );

  const runSetProceduralOperand = useCallback(
    (nodeId: string, port: string, ty: string, label: string) => {
      const raw = procOperands[`${nodeId}:${port}`] ?? '';
      const parts = raw
        .split(',')
        .map((part) => Number(part.trim()))
        .filter((n) => Number.isFinite(n));
      if (ty === 'point') {
        if (parts.length !== 2) {
          appendLog('error', `✗ ${label}.${port}: a point needs two numbers, e.g. 40, 20`);
          return;
        }
        runCommand(
          `Set ${label}.${port} ← (${parts[0]}, ${parts[1]})`,
          setProceduralOperand(nodeId, port, pointOperand(parts[0], parts[1])),
        );
        return;
      }
      if (parts.length !== 1) {
        appendLog('error', `✗ ${label}.${port}: enter one number`);
        return;
      }
      runCommand(
        `Set ${label}.${port} ← ${parts[0]}`,
        setProceduralOperand(nodeId, port, scalarOperand(parts[0])),
      );
    },
    [procOperands, runCommand, appendLog],
  );

  const runToggleProcedural = useCallback(
    (id: string, enabled: boolean, name: string) => {
      runCommand(
        `${enabled ? 'Re-arm' : 'Park'} ${name}`,
        setProceduralEnabled(id, enabled),
      );
    },
    [runCommand],
  );

  const runRemoveProcedural = useCallback(
    (id: string, name: string) => {
      runCommand(`Remove ${name}`, removeProceduralNode(id));
    },
    [runCommand],
  );

  const runToggleConstraint = useCallback(
    (id: string, enabled: boolean) => {
      runConstraint(`${enabled ? 'Enable' : 'Disable'} constraint`, setConstraintEnabled(id, enabled));
    },
    [runConstraint],
  );

  /** Client pixels → document coordinates, asked of the renderer (Task 5.0).
   *  The canvas owns the camera, so the mapping the pointer gets is the mapping
   *  the pixels were drawn with — this file does no transform arithmetic. */
  const docPoint = useCallback(
    (clientX: number, clientY: number): { x: number; y: number } | null =>
      client?.pointerDoc(clientX, clientY) ?? null,
    [client],
  );

  // ── Task 6.0: motion ──────────────────────────────────────────────────
  //
  // Four actions, all of them thin: the anchor, the hit test, the smoothing and
  // the idle signal are the engine's. React sends a slot and two numbers, and
  // reports where the pointer is.

  /** The float slot a hover demo should animate for this primitive. */
  const hoverSlot = useCallback((nodeId: string): string | null => {
    const primitive = snapshot?.scene.nodes[nodeId]?.primitive;
    if (!primitive) return null;
    return primitive.type === 'circle' || primitive.type === 'arc' ? 'radius' : 'width';
  }, [snapshot]);

  /** The engine's own value for that slot — what "unbind" writes back. */
  const slotValue = useCallback((nodeId: string, property: string): number | null => {
    const primitive = snapshot?.scene.nodes[nodeId]?.primitive;
    if (!primitive) return null;
    if (property === 'radius') return 'r' in primitive ? (primitive as { r: number }).r : null;
    if (property === 'width') return 'w' in primitive ? (primitive as { w: number }).w : null;
    if (property === 'height') return 'h' in primitive ? (primitive as { h: number }).h : null;
    return null;
  }, [snapshot]);

  const runBindHover = useCallback(() => {
    if (!client || selected.length !== 1) return;
    const id = selected[0].id;
    const property = hoverSlot(id);
    const off = Number(hoverOff);
    const on = Number(hoverOn);
    if (!property || !Number.isFinite(off) || !Number.isFinite(on)) {
      appendLog('error', '✗ hover spring needs a slot and two finite values');
      return;
    }
    appendLog('cmd', `→ bind hover spring on ${property} (${off} → ${on})`);
    absorb(client, client.bindHoverSpring(id, property, off, on, 170, 26));
  }, [client, selected, hoverSlot, hoverOff, hoverOn, appendLog, absorb]);

  const runToggleHover = useCallback(
    (on: boolean) => {
      if (!client || selected.length !== 1) return;
      const flag = `hover:${selected[0].id}`;
      appendLog('info', `state ← ${flag.slice(0, 14)}… = ${on}`);
      absorb(client, client.setState(flag, on));
    },
    [client, selected, appendLog, absorb],
  );

  const runUnbind = useCallback(
    (row: { nodeId: string; property: string }) => {
      if (!client) return;
      const value = slotValue(row.nodeId, row.property);
      if (value === null) {
        appendLog('error', `✗ no engine value for ${row.property} — cannot unbind safely`);
        return;
      }
      appendLog('cmd', `→ unbind ${row.property} (literal ${value})`);
      absorb(client, client.dispatch(unbindParam(row.nodeId, row.property, value)));
    },
    [client, slotValue, appendLog, absorb],
  );

  const runRemoveTrack = useCallback(
    (trackId: string) => {
      if (!client) return;
      appendLog('cmd', `→ RemoveMotionTrack ${trackId}`);
      absorb(client, client.removeMotionTrack(trackId));
    },
    [client, appendLog, absorb],
  );

  /** A keyframe ramp on a selected node's x — the track half of MES §12. */
  const runRampTrack = useCallback(() => {
    if (!client || selected.length !== 1) return;
    const id = selected[0].id;
    if (!snapshot?.scene.nodes[id]?.position) {
      appendLog('error', '✗ this layer has no x/y slots to animate');
      return;
    }
    const x = snapshot.scene.nodes[id].position?.x ?? 0;
    const track = motionTrack('intro', 'Intro', 'x', [
      [0, x],
      [1.2, x + 160],
      [2.2, x],
    ]);
    appendLog('cmd', '→ SetMotionTrack intro (x: 3 keys)');
    const first = client.setMotionTrack(track);
    logResponse(first);
    if (first.status === 'ok') {
      const bind = client.dispatch(bindTrack(id, 'x', 'intro', 'x'));
      logResponse(bind);
      if (bind.status === 'ok') {
        absorb(client, client.setTime(0));
        return;
      }
    }
    refresh(client);
  }, [client, selected, snapshot, appendLog, logResponse, absorb, refresh]);

  /**
   * Task 6.0: hover. React reports where the pointer is — in *document* units,
   * converted by the renderer's own camera (`pointerDoc`) — and the engine
   * decides what is under it, which nodes are hovered, and whether that means a
   * flip. A move that flips nothing logs nothing and touches no state; a flip
   * names the node it dirtied, like every other mutation.
   */
  const onCanvasMove = useCallback(
    (e: ReactPointerEvent<HTMLCanvasElement>) => {
      if (!client || drag) return;
      const point = docPoint(e.clientX, e.clientY);
      if (!point) return;
      const res = client.pointerMove(point.x, point.y);
      if (res.status === 'error') {
        appendLog('error', `✗ ${res.message}`);
        return;
      }
      if (res.events.length > 0) {
        logResponse(res);
        refresh(client);
        kick();
      }
    },
    [client, drag, docPoint, appendLog, logResponse, refresh, kick],
  );

  /** The pointer left the canvas: no position to report, so the engine is told
   *  just that — a leave is not a place. */
  const onCanvasLeave = useCallback(() => {
    if (!client) return;
    const res = client.pointerLeave();
    if (res.status === 'ok' && res.events.length > 0) {
      logResponse(res);
      refresh(client);
      kick();
    }
  }, [client, logResponse, refresh, kick]);

  /**
   * Task 5.0: a canvas click *names* a layer — the renderer said which one, and
   * the inspector can now edit it. Same two-layer cap as the layer list, so the
   * constraint builders get the pair they expect.
   */
  const pickLayer = useCallback((id: string) => {
    setSelection((prev) => {
      if (prev.includes(id)) return prev;
      return prev.length >= 2 ? [id] : [...prev, id];
    });
  }, []);

  /** The selected layers as the constraint builders see them. */
  /**
   * RULE 3, the whole chain: the pointer goes to the renderer, the renderer's
   * spatial index answers with a `NodeId`, and that id becomes a `BeginDrag`.
   *
   * Note what this function does *not* do: it never tests whether the click was
   * inside a shape (the renderer's containment test decides), never transforms
   * the coordinate (the renderer's camera does), and never picks a node from a
   * list of layers. React reports where the mouse is and obeys the answer —
   * exactly the "dumb remote" of Task 1.4, now with the canvas as the remote's
   * sensor. A miss clears nothing and sends nothing.
   */
  const startDrag = useCallback(
    (e: ReactPointerEvent<HTMLCanvasElement>) => {
      if (!client || drag) return;
      const id = client.pointerHit(e.clientX, e.clientY);
      if (!id) return; // empty space: no node, no gesture, no command
      const position = snapshot?.scene.nodes[id]?.position;
      if (!position) {
        // The renderer hit a node the engine has no movable slots for (an
        // operation's virtual geometry) — name it, but do not invent a gesture.
        pickLayer(id);
        return;
      }
      const point = docPoint(e.clientX, e.clientY);
      if (!point) return;
      e.preventDefault();
      pickLayer(id);
      appendLog('cmd', `→ BeginDrag ${id}`);
      const res = client.dispatch(beginDrag(id));
      logResponse(res);
      if (res.status !== 'ok') {
        refresh(client);
        return;
      }
      setDrag({ id, dx: point.x - position.x, dy: point.y - position.y });
      refresh(client);
    },
    [client, drag, snapshot, docPoint, appendLog, logResponse, refresh, pickLayer],
  );

  /**
   * While a gesture is live the WINDOW owns the pointer: samples go out as they
   * arrive (the solver is ~1.2µs, so no throttling until a measurement demands
   * it) and a release anywhere — outside the canvas included — ends the
   * gesture. Samples are not logged one line per pixel; the refreshed snapshot
   * is what makes the dragged node and its constrained partners move.
   */
  useEffect(() => {
    if (!drag || !client) return;
    const onMove = (e: PointerEvent) => {
      const point = docPoint(e.clientX, e.clientY);
      if (!point) return;
      const res = client.dispatch(updateDrag(drag.id, point.x - drag.dx, point.y - drag.dy));
      if (res.status !== 'ok') {
        appendLog('error', `✗ ${res.message}`);
        return;
      }
      refresh(client);
    };
    const onUp = () => {
      appendLog('cmd', `→ EndDrag ${drag.id}`);
      const res = client.dispatch(endDrag(drag.id));
      logResponse(res);
      refresh(client);
      setDrag(null);
    };
    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', onUp);
    window.addEventListener('pointercancel', onUp);
    return () => {
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', onUp);
      window.removeEventListener('pointercancel', onUp);
    };
  }, [drag, client, docPoint, appendLog, logResponse, refresh]);

  // ── Task 10.1: the drawing suite ──────────────────────────────────────
  //
  // Everything below is *routing*: a pointer event becomes one engine call, and
  // the engine's answer becomes the session's state. The interaction policy lives
  // in `engine/draw/*` (pure and unit-tested); the geometry lives in
  // `vectra-draw`; this file only decides which button the user pressed.

  /** Re-render from the session's current state. */
  const syncDraw = useCallback(() => {
    setDraw({ ...sessionRef.current.state });
  }, []);

  /** Log a drawing reply: the engine's own sentence, or its refusal. */
  const logDraw = useCallback(
    (label: string, reply: DrawReplyWire) => {
      if (reply.ok && reply.snapped) {
        appendLog('info', `✦ ${label}: snapped a ${reply.snapped}`);
        return;
      }
      if (reply.ok) {
        const id = reply.node_id ? ` ${reply.node_id.slice(0, 8)}` : '';
        appendLog('ok', `✓ ${label}${id}`);
        return;
      }
      appendLog('error', `✗ ${label}: ${reply.error ?? 'refused'}`);
    },
    [appendLog],
  );

  /**
   * Re-read the selected path's anchors.
   *
   * The overlay is a *projection* of the document, so it is re-read whenever the
   * document could have changed — after a command, a sample, an undo or a
   * selection. The engine answers with anchors and handles in document space, and
   * the only thing this component adds is the camera.
   */
  // ── Task 10.6: Smart Components + Make Magic ─────────────────────────────

  /**
   * Push the canvas selection into the engine and read back its prose.
   *
   * RULE 2 needs a "this": a prompt about the selection is only meaningful if
   * the engine knows what is selected. The engine writes the sentence, so the
   * bar and the AI prompt can never disagree about the subject.
   */
  const syncSelection = useCallback(
    (c: VectraClient, ids: string[]) => {
      const reply = c.setSelection(ids);
      if (reply) setSelectionProse(reply.prose);
      setComponent(c.componentView());
    },
    [],
  );

  /** Re-inspect the selection after anything that could change its role. */
  const inspectSelection = useCallback(
    (c: VectraClient) => {
      setComponent(c.componentView());
    },
    [],
  );

  /**
   * **Make Magic** (RULE 2): one prompt, structural commands, engine-validated,
   * self-correcting — and answered with a sentence, never JSON (RULE 4).
   */
  const makeMagic = useCallback(
    (prompt: string) => {
      if (!client) return;
      const text = prompt.trim();
      if (!text) return;
      appendLog('cmd', `→ magic ${JSON.stringify(text)}`);
      const envelope = client.aiExecuteWithRetry(text);
      setAiReceipt(envelope);
      if (envelope.status === 'error') {
        setAiText(text);
        appendLog('error', `✗ magic: ${envelope.code} — ${envelope.message}`);
        for (const correction of envelope.corrections) {
          appendLog(
            'info',
            `magic correction ${correction.attempt}: ${correction.error}${
              correction.note ? ` → ${correction.note}` : ''
            }`,
          );
        }
        refresh(client);
        kick();
        return;
      }
      for (const event of envelope.report.events) {
        const line = formatEvent(event);
        appendLog(line.kind, line.text);
      }
      const dirty = dirtyIdsOf(envelope.report.events);
      if (dirty !== null) setLastDirty(dirty);
      // The sentence, not the plan: the panel and the log both read it.
      setMagicReceipt(envelope.prose);
      appendLog('ok', envelope.prose || envelope.headline);
      for (const correction of envelope.report.corrections) {
        appendLog(
          'info',
          `magic self-corrected after attempt ${correction.attempt}: ${correction.error}`,
        );
      }
      refresh(client);
      inspectSelection(client);
      kick();
    },
    [client, appendLog, refresh, inspectSelection, kick],
  );

  /** **Create Component** (RULE 1): group the selection behind a master. */
  const makeComponent = useCallback(() => {
    if (!client || selection.length === 0) return;
    appendLog('cmd', `→ create component from ${selection.length} node(s)`);
    const res = client.createComponent(selection);
    absorb(client, res);
    appendLog('ok', res.prose ?? 'component created');
    inspectSelection(client);
  }, [client, selection, appendLog, absorb, inspectSelection]);

  /** **Icon Studio** (RULE 3): one instance per size, each on its artboard. */
  const generateIconSet = useCallback(
    (master: string) => {
      if (!client) return;
      const sizes = iconSizes
        .split(/[,\s]+/)
        .map((part) => Number.parseFloat(part))
        .filter((size) => Number.isFinite(size) && size >= 4 && size <= 512);
      if (sizes.length === 0) {
        appendLog('error', '✗ icon set needs at least one size (e.g. 16 32 48)');
        return;
      }
      appendLog('cmd', `→ icon set at ${sizes.join(', ')}`);
      const res = client.iconSet(master, sizes);
      absorb(client, res);
      appendLog('ok', res.prose ?? 'icon set generated');
      inspectSelection(client);
    },
    [client, iconSizes, appendLog, absorb, inspectSelection],
  );

  /** The prop sliders (RULE 1). A scaled prop edits its factor, engine-side. */
  const setProp = useCallback(
    (target: string, prop: ComponentPropWire, value: number | string) => {
      if (!client) return;
      const typed =
        prop.ty === 'color'
          ? ({ Color: { Literal: String(value) } } as const)
          : ({ Float: { Literal: Number(value) } } as const);
      const res = client.setComponentProp(target, prop.key, typed);
      absorb(client, res);
      inspectSelection(client);
    },
    [client, absorb, inspectSelection],
  );

  /** Place another instance of the inspected master. */
  const placeInstance = useCallback(
    (master: string) => {
      if (!client) return;
      const res = client.instantiateComponent(master);
      absorb(client, res);
      inspectSelection(client);
    },
    [client, absorb, inspectSelection],
  );

  // The macros are engine data, fetched once — like the procedural palette.
  useEffect(() => {
    if (!client) return;
    setMacros(client.structuralMacros());
  }, [client]);

  // ⌘K / Ctrl-K opens the Make Magic bar; Escape closes it. Registered here
  // rather than on the input, so it works wherever the focus is.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
        event.preventDefault();
        setMagicOpen(true);
        return;
      }
      if (event.key === 'Escape') setMagicOpen(false);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  // The engine hears about the selection whenever it changes.
  useEffect(() => {
    if (!client) return;
    syncSelection(client, selection);
  }, [client, selection, syncSelection]);

  const refreshDraw = useCallback(
    (c: VectraClient) => {
      const session = sessionRef.current;
      const id = session.state.nodeId;
      if (!id) {
        if (session.state.anchors.length > 0) {
          session.absorbOverlay(null, []);
          syncDraw();
        }
        return;
      }
      session.absorbOverlay(id, parseOverlay(c.drawOverlay(id)));
      syncDraw();
    },
    [syncDraw],
  );

  /**
   * **Quick Shape** (RULE 3): the hold fired.
   *
   * The gesture is still live — the user is holding the button — so the stroke has
   * to become a document node *now*. For the brush that is the ordinary
   * `draw_brush_commit`; for the pen it is `draw_pen_commit`. Then the rough stroke
   * goes back to the engine, which recognises the primitive, writes the ideal path
   * and adds the constraint rows that hold it there. Two calls, both of them the
   * engine's, and no geometry in this file.
   */
  const runQuickShape = useCallback(() => {
    if (!client) return;
    const session = sessionRef.current;
    const gesture = session.state.gesture;
    const tool = session.drawTool;
    if (!gesture || !tool) return;
    const stroke = strokePoints(gesture);
    if (stroke.length < 6) {
      session.refuseSnap('the stroke is too short to be a shape');
      syncDraw();
      return;
    }
    session.firedHold();
    syncDraw();
    appendLog('cmd', `→ commit the stroke (${stroke.length} samples), then ask for a Quick Shape`);
    const committed =
      tool === 'brush' ? client.drawBrushCommit(null) : client.drawPenCommit(true, null);
    logDraw(tool === 'brush' ? 'brush commit (hold)' : 'pen commit (hold)', committed);
    if (!committed.ok || !committed.node_id) {
      session.absorbCommit(committed);
      session.refuseSnap(committed.error ?? 'the engine could not commit the stroke');
      syncDraw();
      refresh(client);
      kick();
      return;
    }
    // The snap itself: the engine recognises the primitive, rewrites the path to
    // the ideal geometry, and adds the Task 3.1 constraint rows that hold it.
    const snapped = client.drawQuickShape(committed.node_id, stroke);
    logDraw('Quick Shape', snapped);
    session.absorbCommit(committed);
    if (snapped.ok && snapped.node_id) {
      session.absorbCommit(snapped);
      session.mutate({ nodeId: snapped.node_id, committed: true });
    } else {
      session.refuseSnap(snapped.error ?? 'the stroke is not a circle or a rectangle');
    }
    syncDraw();
    refresh(client);
    refreshDraw(client);
    kick();
  }, [client, appendLog, logDraw, syncDraw, refresh, refreshDraw, kick]);

  /** Arm the hold clock, or disarm it when the stroke cannot snap. */
  const armHold = useCallback(() => {
    if (holdTimer.current !== null) {
      window.clearTimeout(holdTimer.current);
      holdTimer.current = null;
    }
    const session = sessionRef.current;
    const remaining = session.hold.remaining(performance.now());
    if (remaining === null) {
      if (session.state.isHoldingForSnap) {
        session.mutate({ isHoldingForSnap: false });
        syncDraw();
      }
      return;
    }
    if (!session.state.isHoldingForSnap) {
      session.mutate({ isHoldingForSnap: true });
      syncDraw();
    }
    // +16 ms is one frame: the timer fires *after* the hold has matured, so the
    // tick can only ever answer `true`.
    holdTimer.current = window.setTimeout(() => {
      holdTimer.current = null;
      if (sessionRef.current.hold.tick(performance.now())) runQuickShape();
    }, remaining + 16);
  }, [runQuickShape, syncDraw]);

  /** Tool switch: RULE 4's gate flips, and a half-drawn gesture is cancelled. */
  const chooseTool = useCallback(
    (tool: ToolId) => {
      if (!client) return;
      const session = sessionRef.current;
      const state = session.state;
      if (state.isDrawing && state.gesture) {
        const gesture = state.gesture;
        client.drawPointer(
          gesture.tool,
          'cancel',
          gesture.last.x,
          gesture.last.y,
          false,
          false,
          performance.now(),
        );
        session.release();
      }
      if (session.drawTool) {
        // A pen draft abandoned when the user picks another tool: the engine's
        // session is reset so a later click starts a fresh path.
        streamlineRef.current = null;
        client.drawPointer(session.drawTool, 'cancel', 0, 0, false, false, performance.now());
      }
      if (state.activeHandle) client.drawEditCancel();
      if (holdTimer.current !== null) {
        window.clearTimeout(holdTimer.current);
        holdTimer.current = null;
      }
      session.setTool(tool);
      session.mutate({
        draft: null,
        samples: null,
        selectedSlot: null,
        hoveredSlot: null,
        activeHandle: null,
        isHoldingForSnap: false,
        snapNote: null,
        status: designerHint(tool),
      });
      syncDraw();
      refreshDraw(client);
      appendLog('info', `⚒ tool: ${tool}`);
    },
    [client, appendLog, refreshDraw, syncDraw],
  );

  /** A pointer press on the canvas, routed by the active tool. */
  const onCanvasDown = useCallback(
    (e: ReactPointerEvent<HTMLCanvasElement>) => {
      if (!client) return;

      // ── RULE 1 first: the invisible UI answers before the tools do ────
      //
      // A contact is offered to the recogniser before it can become a stroke.
      // One finger draws, exactly as before; a second finger turns the gesture
      // into the invisible UI, cancels the half-drawn stroke (never commits it)
      // and takes the pointer away from the tools until the hand lifts.
      if (e.pointerType === 'touch') {
        const decision = touchRef.current.down(
          { pointerId: e.pointerId, clientX: e.clientX, clientY: e.clientY },
          performance.now(),
        );
        if (decision.swallow) e.preventDefault();
        if (decision.contacts >= 2) {
          const session = sessionRef.current;
          cancelGesture();
          session.mutate({ status: `⇱ ${decision.contacts}-finger gesture…` });
          syncDraw();
          return;
        }
      } else {
        // The mouse's half of RULE 1: Ctrl+Click is Undo, Ctrl+Shift+Click is
        // Redo — armed here, decided on release, because a Ctrl-*drag* is not a
        // click (`modifierClick` reads the travel).
        const action = modifierClick({
          ctrlKey: e.ctrlKey,
          shiftKey: e.shiftKey,
          button: e.button,
        });
        if (action !== 'none') {
          e.preventDefault();
          modifierClickRef.current = { action, pointerId: e.pointerId, x: e.clientX, y: e.clientY };
          return;
        }
      }

      const session = sessionRef.current;
      const point = docPoint(e.clientX, e.clientY);
      if (!point) return;
      const tool = session.drawTool;

      // ── the pen and the brush ────────────────────────────────────────
      if (tool) {
        e.preventDefault();
        // **RULE 2**: this stroke's filter, built from the slider. "Never draw
        // the point immediately": the engine is handed the filtered point from
        // the very first sample, and the first sample of a one-point tap is its
        // own average — so a click still lands exactly where the designer
        // clicked, and the lag appears only once there is a direction to lag
        // behind.
        const filter = new StreamLine(streamlineAmount(streamline));
        streamlineRef.current = filter;
        session.press({
          tool,
          pointerId: e.pointerId,
          point: filter.push(point),
          client: { x: e.clientX, y: e.clientY },
          alt: e.altKey,
          pressure: e.pressure > 0 && e.pressure !== 0.5 ? e.pressure : null,
        });
        syncDraw();
        appendLog(
          'cmd',
          `→ ${tool} down (${point.x.toFixed(1)}, ${point.y.toFixed(1)})${e.altKey ? ' [alt]' : ''}`,
        );
        const gesture = session.state.gesture;
        if (!gesture) return;
        const intent = downIntent(gesture, performance.now());
        if (intent.call !== 'pointer') return;
        const reply = client.drawPointer(
          intent.tool,
          intent.kind,
          intent.x,
          intent.y,
          intent.alt,
          intent.close,
          intent.time,
        );
        logDraw(`${tool} down`, reply);
        session.absorbReply(reply);
        syncDraw();
        armHold();
        return;
      }

      // ── the white arrow: an anchor beats the shape beneath it ─────────
      if (session.state.tool === 'direct') {
        e.preventDefault();
        const current = session.state.nodeId;
        if (current) {
          const hit = parseHit(client.drawHit(current, point.x, point.y));
          const target = editTarget(hit);
          if (target) {
            appendLog(
              'cmd',
              `→ Begin edit ${target.slot}${target.anchor ? '' : ` (${target.side} handle)`}`,
            );
            session.mutate({
              isDrawing: true,
              activeHandle: hit.kind === 'handle' ? { slot: hit.slot, side: 'out' } : null,
              selectedSlot: hit.kind === 'anchor' ? hit.slot : session.state.selectedSlot,
              hoveredSlot: hit.slot,
              status: anchorLabel(session.state.anchors, target.slot),
            });
            syncDraw();
            absorb(client, client.drawEditBegin(current, target.slot, target.side, target.anchor));
            return;
          }
        }
        // Not a vertex of the selected path: the renderer's index decides which
        // path is under the pointer — RULE 4's "clicking a path selects the whole
        // path" is exactly this branch.
        const id = client.pointerHit(e.clientX, e.clientY);
        if (id) {
          pickLayer(id);
          session.mutate({ nodeId: id, selectedSlot: null, status: '◧ path selected' });
          syncDraw();
          refreshDraw(client);
        } else {
          session.mutate({
            nodeId: null,
            anchors: [],
            selectedSlot: null,
            status: '◧ nothing under the pointer',
          });
          syncDraw();
        }
        return;
      }

      // ── the select tool: Task 3.2's drag triad, unchanged ─────────────
      startDrag(e);
    },
    [
      client,
      docPoint,
      appendLog,
      logDraw,
      syncDraw,
      armHold,
      absorb,
      pickLayer,
      refreshDraw,
      startDrag,
    ],
  );

  /** The canvas: hover, or the live stroke between press and release. */
  const onDrawMove = useCallback(
    (e: ReactPointerEvent<HTMLCanvasElement>) => {
      // ── RULE 1 again: a contact that *travels* is the recogniser's first ──
      //
      // Three fingers dragged down is the copy/paste swipe, and it fires the
      // moment the threshold is crossed (a tap waits for the lift; a swipe
      // cannot). From the second contact on, the pointer is not hovering and not
      // drawing: the tools are told nothing until the hand lifts.
      if (e.pointerType === 'touch') {
        const decision = touchRef.current.move(
          { pointerId: e.pointerId, clientX: e.clientX, clientY: e.clientY },
          performance.now(),
        );
        if (decision.swallow) e.preventDefault();
        if (decision.action !== 'none') {
          appendLog('info', `⇱ ${GESTURE_LABEL[decision.action]} → ${decision.action}`);
          runTouchAction(decision.action);
        }
        if (decision.contacts >= 2) return;
      }
      if (!client) return;
      const session = sessionRef.current;
      if (session.state.isDrawing && session.state.gesture) return; // the window owns it
      const point = docPoint(e.clientX, e.clientY);
      if (!point) return;
      if (session.state.tool === 'direct') {
        session.mutate({ cursor: point });
        const current = session.state.nodeId;
        if (current) {
          const hit = parseHit(client.drawHit(current, point.x, point.y));
          const slot = hit.kind === 'miss' ? null : hit.slot;
          if (slot !== session.state.hoveredSlot) session.mutate({ hoveredSlot: slot });
        }
        syncDraw();
        return;
      }
      onCanvasMove(e);
    },
    [client, docPoint, syncDraw, onCanvasMove, appendLog, runTouchAction],
  );

  /**
   * **Finish the pen path** (RULE 2): Enter, or Escape once the path is idle.
   *
   * `close: false` always, because a designer pressing Enter is saying "that is
   * the shape I want" — closing is the click on the first anchor, and nothing
   * else. Escape is two keys in one: *while a gesture is live* it aborts that
   * gesture (the document keeps the anchors placed before it), and *once the
   * pointer is up* it finishes the open path — which is the Illustrator Muscle,
   * where a half-drawn path is one Escape away from being a real one.
   */
  const finishPen = useCallback(
    (close: boolean) => {
      if (!client) return;
      const session = sessionRef.current;
      if (!session.isPen) return;
      const intent = finishIntent();
      if (intent.call !== 'penCommit') return;
      appendLog('cmd', `→ pen commit (close: ${close})`);
      const reply = client.drawPenCommit(close, session.state.nodeId);
      logDraw('pen commit', reply);
      session.absorbCommit(reply);
      syncDraw();
      refresh(client);
      refreshDraw(client);
      kick();
    },
    [client, appendLog, logDraw, syncDraw, refresh, refreshDraw, kick],
  );

  const cancelGesture = useCallback(() => {
    if (!client) return;
    const session = sessionRef.current;
    const state = session.state;
    if (state.activeHandle) {
      appendLog('cmd', '→ cancel the anchor edit');
      absorb(client, client.drawEditCancel());
      session.mutate({ isDrawing: false, activeHandle: null });
      syncDraw();
      refreshDraw(client);
      return;
    }
    const gesture = state.gesture;
    const tool = session.drawTool;
    if (!gesture && !tool) return;
    if (gesture && tool) {
      const intent = cancelIntent(gesture, performance.now());
      if (intent.call === 'pointer') {
        client.drawPointer(
          intent.tool,
          intent.kind,
          intent.x,
          intent.y,
          intent.alt,
          intent.close,
          intent.time,
        );
      }
    }
    if (tool) client.drawPointer(tool, 'cancel', 0, 0, false, false, performance.now());
    session.release();
    session.mutate({ draft: null, samples: null, status: '✗ gesture cancelled' });
    syncDraw();
    refreshDraw(client);
  }, [client, absorb, appendLog, syncDraw, refreshDraw]);

  /**
   * The gesture's **pointer-move**, while a drag is live.
   *
   * The window owns the pointer during a gesture (the same choice Task 3.2's drag
   * makes, and for the same reason: a stroke that leaves the canvas must not stop
   * being a stroke). One engine call per sample — the pen builds its handles from
   * the drag, the brush appends a filtered sample — and the session's own state
   * follows the reply.
   */
  const drawGestureMove = useCallback(
    (e: PointerEvent) => {
      if (!client) return;
      const session = sessionRef.current;
      const state = session.state;
      if (!state.isDrawing) return;
      // A committed stroke (a Quick Shape fired) is no longer being drawn: the
      // pointer is still down, but the geometry belongs to the document now.
      if (state.committed) return;
      const point = docPoint(e.clientX, e.clientY);
      if (!point) return;
      if (state.activeHandle) {
        const res = client.drawEditUpdate(point.x, point.y, e.altKey);
        if (res.status === 'error') {
          appendLog('error', `✗ ${res.message}`);
          return;
        }
        refresh(client);
        refreshDraw(client);
        kick();
        return;
      }
      const gesture = state.gesture;
      const tool = session.drawTool;
      if (!gesture || !tool) return;
      // **RULE 2**: the point the engine sees is the *filtered* one — the raw
      // client position still drives the slop test (a screen-space question),
      // while the smoothing lives in document space, where the stroke does.
      const filter = streamlineRef.current;
      const sample = filter ? filter.push(point) : point;
      session.extend(sample, { x: e.clientX, y: e.clientY }, e.pressure > 0 && e.pressure !== 0.5 ? e.pressure : null);
      session.mutate({ isAltPressed: e.altKey });
      const intent = moveIntent(session.state.gesture ?? gesture, performance.now());
      if (intent.call !== 'pointer') return;
      const reply = client.drawPointer(
        intent.tool,
        intent.kind,
        intent.x,
        intent.y,
        intent.alt,
        intent.close,
        intent.time,
      );
      session.absorbReply(reply);
      syncDraw();
      armHold();
    },
    [
      client,
      docPoint,
      appendLog,
      syncDraw,
      armHold,
      refresh,
      refreshDraw,
      kick,
      cancelGesture,
      streamline,
    ],
  );

  /** The gesture's **release**: commit, or end the anchor edit. */
  const drawGestureUp = useCallback(() => {
    if (!client) return;
    const session = sessionRef.current;
    const state = session.state;
    if (!state.isDrawing) return;
    if (state.activeHandle) {
      appendLog('cmd', '→ End edit');
      absorb(client, client.drawEditEnd());
      session.mutate({ isDrawing: false, activeHandle: null });
      syncDraw();
      refreshDraw(client);
      return;
    }
    // **RULE 2's tail.** The filter lags, so the hand's final position has to be
    // handed over before the release or the stroke ends where the filter was,
    // not where the hand stopped. For the brush it must *reach the engine* (the
    // stroke is fitted from the engine's samples); for the pen the session is
    // enough, because the pen's `up` reports the gesture's last point.
    const filter = streamlineRef.current;
    if (filter && state.gesture) {
      const tail = filter.flush();
      const last = tail[tail.length - 1];
      const sample = state.gesture.samples[state.gesture.samples.length - 1];
      if (last && sample) {
        if (state.gesture.tool === 'brush') {
          client.drawPointer('brush', 'move', last.x, last.y, false, false, performance.now());
        }
        session.extend(last, { x: sample.clientX, y: sample.clientY }, sample.pressure);
      }
    }
    streamlineRef.current = null;

    const gesture = sessionRef.current.state.gesture ?? state.gesture;
    const tool = session.drawTool;
    if (!gesture || !tool) return;
    if (state.committed) {
      // The stroke already became a node when the hold fired: the release only
      // ends the gesture. Committing twice would draw the shape twice.
      session.release();
      session.mutate({ committed: false });
      syncDraw();
      refreshDraw(client);
      return;
    }
    const intent = upIntent(gesture, performance.now());
    appendLog('cmd', `→ ${tool} up${isDrag(gesture) ? ' (dragged)' : ''}`);
    if (intent.call === 'brushCommit') {
      const reply = client.drawBrushCommit(null);
      logDraw('brush commit', reply);
      session.absorbCommit(reply);
      syncDraw();
      refresh(client);
      refreshDraw(client);
      kick();
      return;
    }
    if (intent.call !== 'pointer') return;
    const reply = client.drawPointer(
      intent.tool,
      intent.kind,
      intent.x,
      intent.y,
      intent.alt,
      intent.close,
      intent.time,
    );
    logDraw('pen up', reply);
    session.absorbReply(reply);
    syncDraw();
  }, [client, appendLog, logDraw, syncDraw, absorb, refresh, refreshDraw, kick]);

  /**
   * The gesture lives on the **window**: a stroke that leaves the canvas keeps
   * being a stroke, and a release outside it still ends the gesture. The same
   * contract Task 3.2's node drag uses — and the reason the canvas's own
   * `onPointerMove` stands down while `isDrawing` is true.
   */
  useEffect(() => {
    if (!draw.isDrawing) return;
    window.addEventListener('pointermove', drawGestureMove);
    window.addEventListener('pointerup', drawGestureUp);
    window.addEventListener('pointercancel', drawGestureUp);
    return () => {
      window.removeEventListener('pointermove', drawGestureMove);
      window.removeEventListener('pointerup', drawGestureUp);
      window.removeEventListener('pointercancel', drawGestureUp);
    };
  }, [draw.isDrawing, drawGestureMove, drawGestureUp]);

  // ── Task 10.1: the keyboard ───────────────────────────────────────────
  //
  // Tool shortcuts (V/A/P/B — Illustrator's own letters), Enter to finish a pen
  // path, Escape to cancel it, and Alt tracked so `is_alt_pressed` is honest even
  // before a gesture starts.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (
        target &&
        (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable)
      ) {
        return;
      }
      const session = sessionRef.current;
      const carried = event.altKey !== session.state.isAltPressed;
      if (carried) session.mutate({ isAltPressed: event.altKey });
      if (event.key === 'Escape') {
        event.preventDefault();
        if (sessionRef.current.isPen && !sessionRef.current.state.isDrawing) finishPen(false);
        else cancelGesture();
        return;
      }
      if (event.key === 'Enter') {
        event.preventDefault();
        finishPen(false);
        return;
      }
      if (event.altKey || event.ctrlKey || event.metaKey) return;
      // Task 10.3 RULE 2: the zoom keys every editor shares. `=` is the
      // unshifted `+`, and both are accepted for the same gesture.
      if (event.key === '+' || event.key === '=') {
        event.preventDefault();
        zoomBy(ZOOM_STEP);
        return;
      }
      if (event.key === '-' || event.key === '_') {
        event.preventDefault();
        zoomBy(1 / ZOOM_STEP);
        return;
      }
      if (event.key === '0') {
        event.preventDefault();
        zoomFit();
        return;
      }
      if (event.key === '1') {
        event.preventDefault();
        zoomTo(1);
        return;
      }
      const tool = toolForShortcut(event.key);
      if (tool) {
        event.preventDefault();
        chooseTool(tool);
        return;
      }
      if (carried) syncDraw();
    };
    const onKeyUp = (event: KeyboardEvent) => {
      if (event.key === 'Alt' || !event.altKey) {
        sessionRef.current.mutate({ isAltPressed: event.altKey });
        syncDraw();
      }
    };
    window.addEventListener('keydown', onKeyDown);
    window.addEventListener('keyup', onKeyUp);
    return () => {
      window.removeEventListener('keydown', onKeyDown);
      window.removeEventListener('keyup', onKeyUp);
    };
  }, [chooseTool, cancelGesture, finishPen, syncDraw]);

  /**
   * **Place the overlay** (RULE 4).
   *
   * One camera call per session change: the model is built from engine data
   * (anchors, handles, the draft's points, the brush's samples) and mapped to
   * client pixels by the renderer — the same camera the code above sent the
   * pointer through. The overlay therefore cannot drift from the geometry.
   */
  useEffect(() => {
    if (!client) return;
    const session = sessionRef.current.state;
    const model = overlayModel({
      anchors: session.anchors,
      revealed: revealedHandles(session.anchors, session.hoveredSlot, session.selectedSlot),
      selectedSlot: session.selectedSlot,
      draft: session.draft ? { points: session.draft.points, start: session.draft.start } : null,
      samples: session.samples ?? undefined,
      cursor: session.tool === 'select' ? null : session.cursor,
    });
    if (model.shape.length === 0) {
      setPlaced({ points: [], lines: [] });
      return;
    }
    setPlaced(placeOverlay(model, client.documentToClient(model.shape)));
  }, [client, draw]);

  /** The status line under the canvas: the tool's, or the engine's last word. */
  const drawStatus =
    draw.status ||
    (draw.tool === 'direct'
      ? anchorLabel(draw.anchors, draw.selectedSlot)
      : designerHint(draw.tool));

  const holdNote = draw.isHoldingForSnap ? holdHint(sessionRef.current.hold, performance.now()) : null;

  const ready = status === 'ready' && client !== null;
  const layers = snapshot ? [...snapshot.scene.z_order].reverse() : [];
  const variables = snapshot ? Object.entries(snapshot.variables) : [];
  const expressions = snapshot ? Object.entries(snapshot.expressions) : [];
  const diagnostics = snapshot?.diagnostics ?? [];
  const dirtySet = new Set(lastDirty);
  const constraints = constraintRows(snapshot?.constraints ?? {});
  const operations = operationRows(snapshot?.operations ?? {});
  const layerTree = layerRows(
    snapshot?.scene.z_order ?? [],
    snapshot?.scene.nodes ?? {},
    snapshot?.operations ?? {},
  );
  const proceduralRowsView = proceduralRows(procedural);
  const proceduralPorts = proceduralPortOptions(procedural);
  const proceduralLine = proceduralSummary(procedural);
  const proceduralDirty = (id: string) => dirtySet.has(id);
  const solverLine = snapshot ? solverSummary(snapshot.solver) : '';
  const gestureLine = snapshot
    ? dragStatus(snapshot.solver, snapshot.scene.nodes)
    : null;

  // ── Dependency inspector projection (pure view code, see view-model.ts) ──
  const depRows = dependencyRows(deps, snapshot?.scene.nodes ?? {});
  const graphSummary = deps?.status === 'ok' ? deps.summary : null;

  return (
    <div className="app">
      <header className="header">
        <div className="brand">
          <span className="brand-mark">⚙</span>
          <div>
            <h1>VECTRA</h1>
            <p>Engine remote · Tasks 3.1–3.2 · constraints + drag</p>
          </div>
        </div>
        <div className="header-right">
          {snapshot && (
            <div className="eval-chip" data-testid="eval-chip">
              <span className="eval-mode">{snapshot.eval.last_mode}</span>
              <span className="eval-detail">{evalSummary(snapshot.eval)}</span>
            </div>
          )}
          <div className={`status status-${status}`} data-testid="engine-status">
            <span className="dot" />
            {statusDetail}
          </div>
        </div>
      </header>

      <DocumentBar
        host={host}
        documentJson={() => client?.documentJson() ?? ''}
        onReplay={replayPlan}
        onLog={fileLog}
        ready={client !== null}
      />

      <ArtboardBar
        snapshot={snapshot}
        onCommand={runCommand}
        onFrame={frameDocument}
        onExportCurrent={exportCurrentArtboard}
        onExportAll={exportAllArtboards}
        disabled={!ready}
      />

      <main className="main">
        <section className="panel canvas-panel">
          <h2>
            Canvas{' '}
            <span className="sub">
              WebGPU · the engine tessellates snapshot.scene, the GPU draws it
            </span>
          </h2>
          <ToolPalette
            tool={draw.tool}
            onSelect={chooseTool}
            disabled={!ready}
            status={drawStatus}
          />
          <DrawSettingsPanel
            tool={draw.tool}
            streamline={streamline}
            onStreamline={setStreamline}
            color={dropColor}
            onColor={setDropColor}
            onDropStart={startColorDrop}
            dragging={dropPos !== null}
            disabled={!ready}
          />
          <CanvasPane
            canvasRef={canvasRef}
            onPointerDown={onCanvasDown}
            onPanDown={onPanDown}
            onPanMove={onPanMove}
            onPanUp={onPanUp}
            onPointerMove={onDrawMove}
            onPointerLeave={onCanvasLeave}
            onPointerUp={onCanvasUp}
            onPointerCancel={onCanvasCancel}
            hasNodes={(snapshot?.scene.z_order.length ?? 0) > 0}
            status={canvas}
            frame={frame}
            cursor={pointerCursor(draw.tool, draw.isDrawing)}
            grid={gridStyle(view, zoom)}
            overlay={
              <>
                <NavigationOverlay
                  view={view}
                  zoom={zoom}
                  boards={overlayBoards(snapshot)}
                  toClient={(points) => client?.documentToClient(points) ?? []}
                  onZoomIn={() => zoomBy(ZOOM_STEP)}
                  onZoomOut={() => zoomBy(1 / ZOOM_STEP)}
                  onZoomReset={() => zoomTo(1)}
                  onZoomFit={zoomFit}
                  disabled={!ready}
                />
                <DrawOverlay
                overlay={placed}
                selectedSlot={draw.selectedSlot}
                hoveredSlot={draw.hoveredSlot}
                  holding={draw.isHoldingForSnap}
                  snapNote={draw.snapNote}
                />
              </>
            }
          />
          <div className="drag-strip" data-testid="drag-status">
            {gestureLine ??
              (draw.tool === 'select'
                ? 'Drag a node: the pointer sends BeginDrag / UpdateDrag / EndDrag — the engine moves the geometry.'
                : drawStatus)}
            {holdNote ? <span className="hold-note"> · {holdNote}</span> : null}
          </div>
          <div className="diag-strip" data-testid="diagnostics">
            {diagnostics.length === 0 ? (
              <span className="diag-clean">✓ evaluation clean — no diagnostics</span>
            ) : (
              diagnostics.map((d, i) => (
                <span key={i} className={`diag diag-${d.severity}`}>
                  [{d.severity}] {d.code}: {d.message}
                </span>
              ))
            )}
          </div>
        </section>

        <aside className="side">
          {!showsMathPanels(draw.tool) && (
            <DesignerRail
              tool={draw.tool}
              anchors={draw.anchors.length}
              segments={draw.draft?.kinds.length ?? null}
              selected={anchorLabel(draw.anchors, draw.selectedSlot)}
              status={drawStatus}
            />
          )}
          {/* RULE 1: the Layers Panel is always there — it is the document's
              spine, not a mode. RULE 3: the Appearance Panel appears only with a
              single unlocked object selected. */}
          <LayersPanel
            snapshot={snapshot}
            expanded={expandedLayers}
            onToggleExpanded={(layerId) =>
              setExpandedLayers((prev) => {
                const next = new Set(prev);
                if (next.has(layerId)) next.delete(layerId);
                else next.add(layerId);
                return next;
              })
            }
            onCommand={runCommand}
            onAssignNode={(layerId) => {
              const node = selection[0];
              if (node) runCommand('Move to layer', assignNodeToLayer(node, layerId));
            }}
            selection={selection}
            onGroupSelection={groupSelection}
            pendingNode={selection.length === 1 && showsAppearancePanel(snapshot, selection) ? selection[0] : null}
            disabled={!ready}
          />
          {showsAppearancePanel(snapshot, selection) && (
            <AppearancePanel
              node={selectedNode(snapshot, selection)}
              onCommand={runCommand}
              disabled={!ready}
            />
          )}
          {showsMathPanels(draw.tool) && (
            <>
          <section className="panel">
            <h2>Actions</h2>
            <div className="btn-row">
              <button
                data-testid="add-circle"
                disabled={!ready}
                onClick={() =>
                  runCommand(
                    'Add Circle',
                    createCircle({ cx: 100, cy: 100, r: 50, name: 'circle' }),
                  )
                }
              >
                + Circle
              </button>
              <button
                data-testid="add-rectangle"
                disabled={!ready}
                onClick={() =>
                  runCommand(
                    'Add Rectangle',
                    createRectangle({ x: 200, y: 150, w: 160, h: 100, name: 'rect' }),
                  )
                }
              >
                + Rectangle
              </button>
            </div>
            <div className="btn-row">
              <button
                data-testid="add-bound-rectangle"
                disabled={!ready}
                title="Rectangle whose width reads $base — creating it adds a dependency edge"
                onClick={runAddBoundRectangle}
              >
                ⛓ Rect ← $base
              </button>
              <button
                data-testid="add-bound-circle"
                disabled={!ready}
                title="Circle whose radius reads an expression — depth 2 in the graph"
                onClick={runAddBoundCircle}
              >
                ⛓ Circle ← ƒx
              </button>
            </div>
            <div className="btn-row">
              <button
                data-testid="force-full-reeval"
                disabled={!ready}
                title="Rebuild the whole scene; the incremental result must be identical"
                onClick={runFullReeval}
              >
                ↻ Full re-eval
              </button>
              <span className="hint">patch ≡ rebuild, every time</span>
            </div>
            <div className="btn-row">
              <button
                data-testid="undo"
                disabled={!ready || !snapshot?.can_undo}
                onClick={runUndo}
              >
                ↩ Undo
              </button>
              <button
                data-testid="redo"
                disabled={!ready || !snapshot?.can_redo}
                onClick={runRedo}
              >
                ↪ Redo
              </button>
            </div>
            <label className="time-row">
              <span>time {(snapshot?.time ?? 0).toFixed(1)}s</span>
              <input
                type="range"
                min={0}
                max={10}
                step={0.1}
                value={snapshot?.time ?? 0}
                disabled={!ready}
                onChange={(e) => runSetTime(Number(e.target.value))}
                aria-label="Engine time"
              />
            </label>
          </section>

          {/* ── Task 6.0: motion ───────────────────────────────────────
              Everything here is a report or a single intent. The panel shows
              what the engine says it is doing (`motionSummary`), which flags
              the host currently holds down, and every binding — and the buttons
              send a slot plus two numbers, or toggle a state flag. Nothing in
              this section knows what a spring is. */}
          <section className="panel">
            <h2>
              Motion{' '}
              <span
                className={animating ? 'badge-live' : 'sub'}
                data-testid="motion-status"
              >
                {motionSummary(motion)}
              </span>
            </h2>
            <p className="empty" data-testid="motion-hint">
              {selected.length === 1
                ? `${selected[0].name}: bind a hover spring, or ramp its x with a keyframe track`
                : 'Select one layer to give it motion.'}
            </p>
            <div className="btn-row">
              <label className="field">
                <span>rest</span>
                <input
                  type="number"
                  value={hoverOff}
                  disabled={!ready}
                  onChange={(e) => setHoverOff(e.target.value)}
                  aria-label="Hover rest value"
                />
              </label>
              <label className="field">
                <span>hover</span>
                <input
                  type="number"
                  value={hoverOn}
                  disabled={!ready}
                  onChange={(e) => setHoverOn(e.target.value)}
                  aria-label="Hover value"
                />
              </label>
            </div>
            <div className="btn-row">
              <button
                data-testid="motion-bind"
                disabled={!ready || selected.length !== 1}
                title="Bind a spring that eases toward the hover value while the pointer is over the shape"
                onClick={runBindHover}
              >
                Bind hover spring
              </button>
              <label className="check" title="Hold the hover flag by hand instead of moving the pointer">
                <input
                  type="checkbox"
                  data-testid="motion-hover"
                  disabled={!ready || selected.length !== 1}
                  checked={
                    selected.length === 1 &&
                    (motion?.states.includes(`hover:${selected[0].id}`) ?? false)
                  }
                  onChange={(e) => runToggleHover(e.target.checked)}
                />
                <span>hover</span>
              </label>
            </div>
            <div className="btn-row">
              <button
                data-testid="motion-track"
                disabled={!ready || selected.length !== 1}
                title="Register a three-key x ramp and bind the slot to it"
                onClick={runRampTrack}
              >
                Ramp x (track)
              </button>
              <button
                data-testid="motion-reeval"
                disabled={!ready}
                title="Rebuild the whole scene from scratch (patch ≡ rebuild)"
                onClick={runFullReeval}
              >
                Re-evaluate all
              </button>
            </div>
            {motion && motion.tracks.length > 0 && (
              <div className="chips" data-testid="motion-tracks">
                {motion.tracks.map((id) => (
                  <button
                    key={id}
                    className="chip"
                    title={`Remove track ${id}`}
                    onClick={() => runRemoveTrack(id)}
                  >
                    ♪ {id} ✕
                  </button>
                ))}
              </div>
            )}
            {stateChips(motion).length > 0 && (
              <div className="chips" data-testid="motion-states">
                {stateChips(motion).map((chip) => (
                  <span key={chip} className="chip">
                    ◉ {chip}
                  </span>
                ))}
              </div>
            )}
            <ul className="motion" data-testid="motion-bindings">
              {motionRows(motion).map((row) => (
                <li key={`${row.nodeId}:${row.property}`}>
                  <span className={row.active ? 'motion-live' : 'motion-rest'}>
                    {row.kind === 'spring' ? '≈' : row.kind === 'track' ? '♪' : '◉'}
                  </span>
                  <span className="motion-detail">{row.detail}</span>
                  <span className="motion-value">{row.value}</span>
                  <button
                    className="icon-btn"
                    data-testid={`motion-unbind-${row.property}`}
                    title={`Unbind ${row.property} (writes the engine's current value)`}
                    onClick={() => runUnbind({ nodeId: row.nodeId, property: row.property })}
                  >
                    ✕
                  </button>
                </li>
              ))}
            </ul>
          </section>

          <section className="panel">
            <h2>Layers <span className="sub">{layers.length}</span></h2>
            {layers.length === 0 ? (
              <p className="empty">No layers yet.</p>
            ) : (
              <ul className="layers" data-testid="layers">
                {layerTree.map((row) => {
                  const n = snapshot!.scene.nodes[row.id];
                  const picked = selection.includes(row.id);
                  // Task 4.0: an operation row is nested under the source it
                  // reads and is NOT an operand itself (Phase 1 keeps
                  // operations one level deep), so it is not selectable.
                  return (
                    <li
                      key={row.id}
                      data-testid={`layer-${row.id}`}
                      data-op={row.isOperation ? 'true' : undefined}
                      aria-selected={picked}
                      title={
                        row.isOperation
                          ? `${row.operands} — a virtual node over its sources; the sources stay editable`
                          : 'Click to select for a constraint or an operation (two layers)'
                      }
                      className={[
                        'layer',
                        row.isOperation ? 'layer-op' : '',
                        row.depth > 0 ? 'layer-nested' : '',
                        dirtySet.has(row.id) ? 'layer-dirty' : '',
                        picked ? 'layer-selected' : '',
                      ]
                        .filter(Boolean)
                        .join(' ')}
                      onClick={() => {
                        if (!row.isOperation) toggleSelection(row.id);
                      }}
                    >
                      <span className="kind-tag">
                        {row.isOperation ? row.tag : n?.primitive.type}
                      </span>
                      <span className="layer-name">
                        {row.isOperation && <span className="op-glyph">{row.operands}</span>}
                        {row.name}
                      </span>
                      {picked && (
                        <span className="pick-chip">{selection[0] === row.id ? '1' : '2'}</span>
                      )}
                      {dirtySet.has(row.id) && (
                        <span className="dirty-chip" title="re-evaluated by the last edit">
                          ⚡
                        </span>
                      )}
                      <span className="layer-id">{row.id.slice(0, 8)}</span>
                      <button
                        className="icon-btn"
                        title={
                          row.isOperation
                            ? `Remove ${row.name} (the sources stay)`
                            : `Delete ${row.name}`
                        }
                        onClick={() =>
                          row.isOperation
                            ? runRemoveOperation(row.id, row.name)
                            : runCommand(`DeleteNode ${row.name}`, deleteNode(row.id))
                        }
                      >
                        ✕
                      </button>
                    </li>
                  );
                })}
              </ul>
            )}
          </section>

          <section className="panel">
            <h2>
              Operations{' '}
              <span className="sub" data-testid="operation-count">
                {operations.length}
              </span>
            </h2>
            <p className="empty" data-testid="operation-hint">
              {pairReady
                ? `${selected[0].name} ${'is the left operand'} — Subtract removes ${selected[1].name} from it`
                : selected.length === 1
                  ? `Modify ${selected[0].name}, or click a second layer for a boolean`
                  : 'Click two Layers rows for a boolean, or one for a modifier.'}
            </p>
            <div className="btn-row">
              <button
                data-testid="op-union"
                disabled={!ready || !pairReady}
                title="Union of the two selected sources"
                onClick={() => runBoolean('union')}
              >
                Union
              </button>
              <button
                data-testid="op-subtract"
                disabled={!ready || !pairReady}
                title="Remove the second source from the first"
                onClick={() => runBoolean('subtract')}
              >
                Subtract
              </button>
              <button
                data-testid="op-intersect"
                disabled={!ready || !pairReady}
                title="Keep only the overlap"
                onClick={() => runBoolean('intersect')}
              >
                Intersect
              </button>
              <button
                data-testid="op-exclude"
                disabled={!ready || !pairReady}
                title="Keep the symmetric difference"
                onClick={() => runBoolean('exclude')}
              >
                Exclude
              </button>
            </div>
            <div className="btn-row">
              <label className="field-inline">
                <span>value</span>
                <input
                  type="number"
                  step={0.5}
                  value={modValue}
                  disabled={!ready}
                  onChange={(e) => setModValue(e.target.value)}
                  aria-label="Modifier operand"
                  data-testid="op-value"
                />
              </label>
              <button
                data-testid="op-offset"
                disabled={!ready || selected.length !== 1}
                title="Offset the selected source's outline by the value"
                onClick={() => runModifier('offset')}
              >
                Offset
              </button>
              <button
                data-testid="op-fillet"
                disabled={!ready || selected.length !== 1}
                title="Round the selected source's corners to the value"
                onClick={() => runModifier('fillet')}
              >
                Fillet
              </button>
              <button
                data-testid="op-mirror"
                disabled={!ready || selected.length !== 1}
                title="Reflect the selected source across the value on x"
                onClick={() => runModifier('mirror')}
              >
                Mirror
              </button>
            </div>
            {operations.length === 0 ? (
              <p className="empty">
                No operations. Sources stay exactly as authored — an operation is a virtual
                result computed from them.
              </p>
            ) : (
              <ul className="ops" data-testid="operations">
                {operations.map((op) => (
                  <li
                    key={op.id}
                    data-testid={`operation-${op.id}`}
                    className={[
                      'op-row',
                      dirtySet.has(op.id) ? 'layer-dirty' : '',
                      op.enabled ? '' : 'op-parked',
                    ]
                      .filter(Boolean)
                      .join(' ')}
                  >
                    <span className="kind-tag">{op.kind}</span>
                    <span className="op-desc" title={`${op.kind} over ${op.inputs.length} source(s)`}>
                      {op.glyph} {op.description}
                    </span>
                    <span className="op-operands">{op.operands}</span>
                    <span className="layer-id">{op.id.slice(0, 8)}</span>
                    <label className="op-toggle" title={op.enabled ? 'Park' : 'Enable'}>
                      <input
                        type="checkbox"
                        checked={op.enabled}
                        onChange={(e) => runToggleOperation(op.id, e.target.checked, op.name)}
                        aria-label={`Enable ${op.name}`}
                      />
                    </label>
                    <button
                      className="icon-btn"
                      title={`Remove ${op.name}`}
                      onClick={() => runRemoveOperation(op.id, op.name)}
                    >
                      ✕
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </section>

          <section className="panel">
            <h2>
              Procedural{' '}
              <span className="sub" data-testid="procedural-count">
                {proceduralRowsView.length}
              </span>
            </h2>
            <p className="empty" data-testid="procedural-summary">
              {proceduralLine}
            </p>
            <div className="btn-row">
              <label className="field-inline">
                <span>kind</span>
                <select
                  value={procKind}
                  disabled={!ready}
                  onChange={(e) => setProcKind(e.target.value)}
                  aria-label="Procedural kind"
                  data-testid="proc-kind"
                >
                  {palette.map((option) => (
                    <option key={option.tag} value={option.tag}>
                      {option.label}
                    </option>
                  ))}
                </select>
              </label>
              {procOption?.needs_subject ? (
                <label className="field-inline">
                  <span>reads</span>
                  <select
                    value={procSubject}
                    disabled={!ready}
                    onChange={(e) => setProcSubject(e.target.value)}
                    aria-label="Source subject"
                    data-testid="proc-subject"
                  >
                    <option value="">— pick a layer —</option>
                    {layers.map((id) => (
                      <option key={id} value={id}>
                        {snapshot?.scene.nodes[id]?.name ?? id.slice(0, 8)}
                      </option>
                    ))}
                  </select>
                </label>
              ) : null}
              <button
                data-testid="proc-add"
                disabled={!ready || !procOption}
                title="Add the selected kind as a node"
                onClick={runAddProceduralNode}
              >
                Add node
              </button>
            </div>
            {proceduralRowsView.length === 0 ? (
              <p className="empty">
                No nodes. A procedural node is live geometry: a grid, a repeat, a
                noise field or a smoother, wired from a layer and composed into the
                scene like any authored shape.
              </p>
            ) : (
              <ul className="ops procedural" data-testid="procedural">
                {proceduralRowsView.map((row) => (
                  <li
                    key={row.id}
                    data-testid={`proc-${row.id}`}
                    className={[
                      'op-row',
                      'proc-row',
                      row.enabled ? '' : 'op-parked',
                      proceduralDirty(row.id) ? 'layer-dirty' : '',
                    ]
                      .filter(Boolean)
                      .join(' ')}
                  >
                    <div className="proc-head">
                      <span className="kind-tag">{row.kind}</span>
                      <span className="op-desc" title={row.description}>
                        {row.name}
                      </span>
                      <span className="op-operands">{row.description}</span>
                      <span className="layer-id">{row.short}</span>
                      <label className="op-toggle" title={row.enabled ? 'Park' : 'Re-arm'}>
                        <input
                          type="checkbox"
                          checked={row.enabled}
                          onChange={(e) => runToggleProcedural(row.id, e.target.checked, row.name)}
                          aria-label={`Enable ${row.name}`}
                        />
                      </label>
                      <button
                        className="icon-btn"
                        title={`Remove ${row.name}`}
                        onClick={() => runRemoveProcedural(row.id, row.name)}
                      >
                        ✕
                      </button>
                    </div>
                    <div className="proc-ports" data-testid={`proc-ports-${row.id}`}>
                      {row.outputs
                        .map((port) => `${port.port} → ${port.value ?? '—'}`)
                        .join(' · ')}
                    </div>
                    {row.inputs.length > 0 || row.operands.length > 0 ? (
                      <div className="proc-controls">
                        {row.inputs.map((input) => (
                          <div className="proc-control" key={input.port}>
                            <span className="proc-port" title={`${input.ty} input`}>
                              {input.port}
                              {input.required ? ' *' : ''}
                              {input.wired ? ' ✓' : ''}
                            </span>
                            <select
                              value={procWiring[`${row.id}:${input.port}`] ?? ''}
                              disabled={!ready}
                              onChange={(e) =>
                                setProcWiring((prev) => ({
                                  ...prev,
                                  [`${row.id}:${input.port}`]: e.target.value,
                                }))
                              }
                              aria-label={`${row.name} ${input.port} source`}
                              data-testid={`proc-wire-${row.id}-${input.port}`}
                            >
                              <option value="">— source port —</option>
                              {proceduralPorts
                                .filter((option) => option.nodeId !== row.id)
                                .map((option) => (
                                  <option key={option.value} value={option.value}>
                                    {option.label} ({option.ty})
                                  </option>
                                ))}
                            </select>
                            <button
                              disabled={!ready}
                              title={`Wire ${input.port}`}
                              onClick={() =>
                                runConnectProcedural(
                                  row.id,
                                  input.port,
                                  procWiring[`${row.id}:${input.port}`] ?? '',
                                  row.name,
                                )
                              }
                            >
                              Wire
                            </button>
                            {input.wired ? (
                              <button
                                className="icon-btn"
                                title={`Unwire ${input.port}`}
                                onClick={() => runDisconnectProcedural(row.id, input.port, row.name)}
                              >
                                ⨯
                              </button>
                            ) : null}
                          </div>
                        ))}
                        {row.operands.map((operand) => (
                          <div className="proc-control" key={operand.port}>
                            <span
                              className="proc-port"
                              title={`${operand.ty} operand — now ${operand.text}`}
                            >
                              {operand.port} = {operand.text}
                            </span>
                            <input
                              className="proc-input"
                              value={procOperands[`${row.id}:${operand.port}`] ?? ''}
                              placeholder={operand.ty === 'point' ? 'x, y' : 'value'}
                              disabled={!ready}
                              onChange={(e) =>
                                setProcOperands((prev) => ({
                                  ...prev,
                                  [`${row.id}:${operand.port}`]: e.target.value,
                                }))
                              }
                              aria-label={`${row.name} ${operand.port} value`}
                              data-testid={`proc-operand-${row.id}-${operand.port}`}
                            />
                            <button
                              disabled={!ready}
                              title={`Set ${operand.port}`}
                              onClick={() =>
                                runSetProceduralOperand(row.id, operand.port, operand.ty, row.name)
                              }
                            >
                              Set
                            </button>
                          </div>
                        ))}
                      </div>
                    ) : null}
                  </li>
                ))}
              </ul>
            )}
            {procedural && procedural.diagnostics.length > 0 ? (
              <ul className="proc-diagnostics" data-testid="procedural-diagnostics">
                {procedural.diagnostics.map((note, index) => (
                  <li key={`${note.code}-${index}`} title={note.code}>
                    {note.message}
                  </li>
                ))}
              </ul>
            ) : null}
          </section>

          <section className="panel">
            <h2>
              Constraints{' '}
              <span className="sub">
                {constraints.length} · {solverLine}
              </span>
            </h2>
            <p className="empty" data-testid="constraint-hint">
              {pairReady
                ? `Constrain ${selected[0].name} ↔ ${selected[1].name} (${selected[0].name} is the anchor)`
                : 'Click two Layers rows to pick a pair.'}
            </p>
            <div className="btn-row">
              <button
                data-testid="add-vertical-constraint"
                disabled={!ready || !pairReady}
                title="Enforce x(node₂) == x(node₁)"
                onClick={runAddVertical}
              >
                Vertical
              </button>
              <button
                data-testid="add-coincident-constraint"
                disabled={!ready || !pairReady}
                title="Two rules: the centres coincide on both axes"
                onClick={runAddCoincident}
              >
                Coincident
              </button>
              <button
                data-testid="add-distance-constraint"
                disabled={!ready || !pairReady}
                title="Capture the current separation on x and hold it"
                onClick={() => runAddDistance()}
              >
                Hold distance
              </button>
              <button
                data-testid="over-constrain"
                disabled={!ready || constraints.length === 0}
                title="Add the same separation at +100, weakly: the engine drops the weaker rule and logs a diagnostic"
                onClick={runOverConstrain}
              >
                Over-constrain
              </button>
            </div>
            {constraints.length === 0 ? (
              <p className="empty">No constraints yet — the solver is idle.</p>
            ) : (
              <ul className="constraints" data-testid="constraint-list">
                {constraints.map((c) => (
                  <li
                    key={c.id}
                    data-testid={`constraint-${c.id}`}
                    className={c.dropped ? 'constraint-dropped' : ''}
                  >
                    <span className="kind-tag">{c.kind}</span>
                    <span className="constraint-detail" title={c.detail}>
                      {c.detail}
                    </span>
                    <span className="strength-chip">{c.meta.split(' · ')[1]}</span>
                    {c.dropped && <span className="dropped-chip">dropped</span>}
                    <button
                      className="icon-btn"
                      title={c.enabled ? 'Disable (park the rule)' : 'Enable'}
                      onClick={() => runToggleConstraint(c.id, !c.enabled)}
                    >
                      {c.enabled ? '⏸' : '▶'}
                    </button>
                    <button
                      className="icon-btn"
                      data-testid={`remove-constraint-${c.id}`}
                      title="Remove"
                      onClick={() =>
                        runConstraint('Remove constraint', removeConstraint(c.id))
                      }
                    >
                      ✕
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </section>

          <section className="panel">
            <h2>Variables <span className="sub">{variables.length}</span></h2>
            {variables.length === 0 ? (
              <p className="empty">No variables yet.</p>
            ) : (
              <ul className="vars" data-testid="variables">
                {variables.map(([name, value]) => (
                  <li key={name}>
                    <span className="var-name">${name}</span>
                    <span className="var-value">{value}</span>
                  </li>
                ))}
              </ul>
            )}
            <div className="var-form">
              <input
                value={varName}
                onChange={(e) => setVarName(e.target.value)}
                placeholder="name"
                aria-label="Variable name"
              />
              <input
                value={varValue}
                onChange={(e) => setVarValue(e.target.value)}
                placeholder="value"
                inputMode="decimal"
                aria-label="Variable value"
              />
              <button data-testid="set-variable" disabled={!ready} onClick={runSetVariable}>
                Set
              </button>
            </div>
          </section>

          <section className="panel">
            <h2>Expressions <span className="sub">ƒx · {expressions.length}</span></h2>
            {expressions.length === 0 ? (
              <p className="empty">No expressions yet.</p>
            ) : (
              <ul className="vars" data-testid="expressions">
                {expressions.map(([id, source]) => (
                  <li key={id}>
                    <span className="layer-id">{id.slice(0, 8)}</span>
                    <span className="var-name">{source}</span>
                    <button
                      className="icon-btn"
                      title={`Remove ${source}`}
                      onClick={() =>
                        runCommand(`RemoveExpression ${source}`, removeExpression(id))
                      }
                    >
                      ✕
                    </button>
                  </li>
                ))}
              </ul>
            )}
            <div className="var-form">
              <input
                value={exprSource}
                onChange={(e) => setExprSource(e.target.value)}
                placeholder="$base * 2 + 10"
                aria-label="Expression source"
                data-testid="expression-source"
              />
              <button
                data-testid="define-expression"
                disabled={!ready}
                onClick={runDefineExpression}
              >
                ƒx Define
              </button>
            </div>
          </section>
            </>
          )}
        </aside>

        {showsMathPanels(draw.tool) && (
        <div className="bottom">
        <section className="panel deps-panel">
          <h2>
            Dependencies{' '}
            <span className="sub">
              {graphSummary
                ? `${graphSummary.nodes} vertices · ${graphSummary.edges} edges · ${
                    graphSummary.acyclic ? 'acyclic ✓' : 'CYCLIC ✗'
                  }`
                : 'engine graph'}
            </span>
          </h2>
          {depRows.length === 0 ? (
            <p className="empty">
              No dependencies yet — add a “⛓ Rect ← $base” or bind a parameter to
              see edges appear.
            </p>
          ) : (
            <ul className="deps" data-testid="dependencies">
              {depRows.map((e) => (
                <li key={`${e.fromKey}->${e.toKey}`}>
                  <span className="dep-from">{e.from}</span>
                  <span className="dep-arrow">→ depends on →</span>
                  <span className="dep-to">{e.to}</span>
                </li>
              ))}
            </ul>
          )}
        </section>

        <section className="panel log-panel">
          <h2>Engine events <span className="sub">command → events round-trip</span></h2>
          <div className="log" ref={logRef} data-testid="event-log">
            {log.map((e) => (
              <div key={e.seq} className={`log-line log-${e.kind}`}>
                <span className="log-seq">{e.seq}</span>
                <span>{e.text}</span>
              </div>
            ))}
          </div>
        </section>

        <section className="panel ai-panel" data-testid="ai-panel">
          <h2>
            AI Assistant{' '}
            <span className="sub" data-testid="ai-summary">
              {ai.summary}
            </span>
            <kbd className="ai-kbd">⌘K</kbd>
          </h2>
          <p className="empty">
            Describe an edit in words. The AI answers with JSON commands — the same
            commands the buttons above send — shows them, and runs them only when you
            say so. It never draws: every command goes through the engine's own
            validation, and a refusal is fed back for a correction.
          </p>
          <div className="btn-row">
            <input
              ref={aiPromptRef}
              data-testid="ai-prompt"
              className="ai-input"
              value={aiText}
              placeholder="round the corners of card by 8"
              onChange={(event) => setAiText(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === 'Enter') {
                  event.preventDefault();
                  aiGenerate();
                }
              }}
            />
            <button
              data-testid="ai-generate"
              disabled={!ready}
              title="Ask for commands and show them (nothing is applied)"
              onClick={aiGenerate}
            >
              Generate
            </button>
            <button
              data-testid="ai-execute"
              disabled={!ready || ai.plan.length === 0}
              title="Run the previewed plan exactly as shown"
              onClick={() => aiExecute(false)}
            >
              Execute
            </button>
            <button
              data-testid="ai-autocorrect"
              disabled={!ready}
              title="Generate, validate, feed the error back, retry up to twice"
              onClick={() => aiExecute(true)}
            >
              Auto-correct
            </button>
          </div>
          {aiPhrasings.length ? (
            <p className="ai-hint-line" data-testid="ai-phrasings">
              understands: {aiPhrasings.join(' · ')}
            </p>
          ) : null}
          {ai.prose ? (
            <p className="magic-prose" data-testid="ai-prose">
              {ai.prose}
            </p>
          ) : null}
          {ai.plan.length ? (
            /* RULE 4: the plan is described in words — "Add vertical constraint",
               "Set width" — never as raw JSON. */
            <ol className="ai-plan" data-testid="ai-plan">
              {ai.plan.map((row) => (
                <li key={row.index} title={row.label}>
                  <span className="ai-plan-index">{row.index}</span>
                  <span className="ai-plan-tag">{row.label}</span>
                  {row.target ? <span className="ai-plan-target">{row.target}</span> : null}
                </li>
              ))}
            </ol>
          ) : null}
          {ai.dirty.length ? (
            <p className="ai-hint-line" data-testid="ai-dirty">
              re-evaluated {ai.dirty.length} node(s): {ai.dirty.join(', ')}
            </p>
          ) : null}
          {ai.corrections.length ? (
            <ul className="ai-corrections" data-testid="ai-corrections">
              {ai.corrections.map((row, index) => (
                <li key={`${row.attempt}-${index}`}>
                  <span className="ai-plan-tag">attempt {row.attempt}</span>
                  <span className="ai-correction-code">{row.code}</span>
                  <span>{row.error}</span>
                  {row.note ? <em> → {row.note}</em> : null}
                </li>
              ))}
            </ul>
          ) : null}
          {ai.notes.length ? (
            <p className="ai-hint-line" data-testid="ai-notes">
              {ai.notes.join(' · ')}
            </p>
          ) : null}
          <div className="btn-row">
            <button
              data-testid="ai-prompt-toggle"
              disabled={!ready}
              onClick={toggleAiPrompt}
            >
              {aiShowPrompt ? 'Hide' : 'Show'} system prompt
            </button>
            <span className="sub" data-testid="ai-context">
              {summaryCounts(aiSummary)}
            </span>
          </div>
          {aiShowPrompt ? (
            <>
              <p className="ai-hint-line" data-testid="ai-grounding">
                grounded on: {summaryLabels(aiSummary).join(' · ') || 'an empty canvas'}
              </p>
              <pre className="ai-system-prompt" data-testid="ai-system-prompt">
                {aiPromptText}
              </pre>
            </>
          ) : null}
        </section>

        {/* ── Task 10.6 RULE 1: the Smart Component inspector ─────────────── */}
        <ComponentPanel
          component={component}
          selectionCount={selection.length}
          ready={ready}
          // The probe is the client's; the sentence is the engine's own words.
          engineIsOlder={Boolean(client?.engineIsOlder)}
          staleSentence={VectraClient.STALE_ENGINE}
          iconSizes={iconSizes}
          onIconSizes={setIconSizes}
          onCreate={makeComponent}
          onPlaceInstance={placeInstance}
          onGenerateIconSet={generateIconSet}
          onSetProp={setProp}
        />

        <section className="panel">
          <h2>
            Export{' '}
            <span className="sub" data-testid="export-summary">
              {exportSummary(exportResult)}
            </span>
          </h2>
          <p className="empty">
            The engine writes the file: a semantic SVG (a circle stays a circle) or a
            parametric React component ({'{'}width={'{'}base * 2{'}'}{'}'}).
          </p>
          <div className="btn-row">
            <select
              data-testid="export-format"
              value={exportFormat}
              onChange={(event) => setExportFormat(event.target.value)}
            >
              <option value="svg">SVG (semantic)</option>
              <option value="react">React (parametric)</option>
            </select>
            <button
              data-testid="export-run"
              disabled={!ready}
              title="Compile the drawing to code"
              onClick={runExport}
            >
              Export
            </button>
          </div>
        </section>
        </div>
        )}
      </main>

      {exportResult ? (
        <div className="export-modal" data-testid="export-modal" role="dialog">
          <div className="export-sheet">
            <header>
              <strong>
                {exportResult.format === 'react' ? 'React component' : 'SVG document'}
              </strong>
              <span className="sub">{exportResult.warnings.length} warning(s)</span>
              <button
                data-testid="export-close"
                onClick={() => setExportResult(null)}
                title="Close"
              >
                ✕
              </button>
            </header>
            {exportResult.warnings.length ? (
              <ul className="export-warnings" data-testid="export-warnings">
                {exportResult.warnings.map((warning, index) => (
                  <li key={index}>{warning}</li>
                ))}
              </ul>
            ) : null}
            <pre className="export-code" data-testid="export-code">
              {exportResult.code}
            </pre>
            <footer>
              <button
                data-testid="export-copy"
                disabled={exportResult.status !== 'ok' || !exportResult.code}
                onClick={() => void copyExport()}
              >
                Copy to Clipboard
              </button>
              <span className="sub">
                {exportResult.status === 'ok'
                  ? exportSummary(exportResult)
                  : exportResult.message}
              </span>
            </footer>
          </div>
        </div>
      ) : null}

      {/* ── Task 10.7 RULE 4: the colour in flight ────────────────────── */}
      {dropPos && (
        <div
          className="colordrop-ghost"
          data-testid="colordrop-ghost"
          style={{ left: dropPos.x, top: dropPos.y, background: dropColor }}
        />
      )}

      {/* ── Task 10.6 RULE 2 + RULE 4: Make Magic (⌘K) ──────────────────── */}
      <MagicBar
        open={magicOpen}
        ready={ready}
        text={magicText}
        onText={setMagicText}
        selectionProse={selectionProse}
        macros={macros}
        receipt={magicReceipt}
        onRun={makeMagic}
        onClose={() => setMagicOpen(false)}
      />
    </div>
  );
}
