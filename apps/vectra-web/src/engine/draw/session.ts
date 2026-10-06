/**
 * **The drawing session** (Task 10.1 §2 of the execution plan: *"Track
 * `is_drawing`, `active_handle`, `last_point`, `is_alt_pressed`,
 * `is_holding_for_snap`, etc."*).
 *
 * This is where the named state lives — and, more importantly, where it is kept
 * **honest**. Every field below is either
 *
 * * something only the browser knows (`is_alt_pressed`, the pointer id, the
 *   clock), or
 * * a *mirror* of something the engine owns (the draft, the anchors, the selected
 *   slot), refreshed from the reply that carried it.
 *
 * Nothing here is a second copy of the document. The draft is whatever
 * `draw_pointer` last answered, the anchors are whatever `draw_overlay` last
 * answered, and the selection is a slot name the overlay validated. That is the
 * Task 1.4 rule surviving contact with a tool that *feels* stateful: it feels
 * stateful because the engine's state is rendered the instant it changes, not
 * because the UI guessed.
 *
 * The session is deliberately not a React hook. It is a plain object with a
 * `mutate` method, so `App.tsx` can hold it in a ref and drive it from pointer
 * handlers and a timer without a render loop of its own — and so the whole thing
 * can be exercised in `tests/view-model.test.ts` with no DOM at all.
 */

import type { AnchorWire, DrawReplyWire } from '../wire';
import { beginGesture, extendGesture, type DrawTool, type Gesture, type Point } from './pointer';
import { SnapHold } from './quick-shape';
import type { Hit } from './path-edit';
import type { ToolId } from './tools';

/** Everything the drawing tools track. */
export interface DrawSessionState {
  /** The tool the user is holding. */
  tool: ToolId;
  /** Is a pointer gesture in flight? (The plan's `is_drawing`.) */
  isDrawing: boolean;
  /** The live gesture, if any. */
  gesture: Gesture | null;
  /** Is Alt/Option down *now*? Sampled on every event, so a modifier change
   *  mid-drag is visible — the gesture itself keeps the value it started with. */
  isAltPressed: boolean;
  /** The pen path or brush stroke so far, exactly as the engine described it. */
  draft: DrawReplyWire['draft'] | null;
  /** The brush's raw samples, for the live preview. */
  samples: [number, number, number][] | null;
  /** The node the tools are working on (the pen's rewrite target, the white
   *  arrow's subject). The engine's `last_node` mirrors it. */
  nodeId: string | null;
  /** The path's anchors, from `draw_overlay` — the overlay's whole vocabulary. */
  anchors: AnchorWire[];
  /** The anchor the white arrow owns, or `null` for "the whole path". */
  selectedSlot: string | null;
  /** The anchor or handle under the pointer. */
  hoveredSlot: string | null;
  /** The handle currently being dragged, if the gesture is a handle drag. */
  activeHandle: { slot: string; side: 'in' | 'out' } | null;
  /** Is the snap timer armed? (The plan's `is_holding_for_snap`.) */
  isHoldingForSnap: boolean;
  /**
   * Has the stroke already become a document node?
   *
   * Set when a Quick Shape fires *mid-gesture* — the user is still holding the
   * button, but the geometry belongs to the document and further samples must not
   * be appended to it (nor committed a second time on release). It is the one
   * piece of state that exists purely because a hold happens *during* a drag.
   */
  committed: boolean;
  /** The last snap's sentence, or a refusal's reason. */
  snapNote: string | null;
  /** The last thing the engine said about a gesture, for the status strip. */
  status: string;
  /** The pointer, in document units — the tool cursor the overlay draws. */
  cursor: Point | null;
}

export function initialSession(tool: ToolId = 'select'): DrawSessionState {
  return {
    tool,
    isDrawing: false,
    gesture: null,
    isAltPressed: false,
    draft: null,
    samples: null,
    nodeId: null,
    anchors: [],
    selectedSlot: null,
    hoveredSlot: null,
    activeHandle: null,
    isHoldingForSnap: false,
    committed: false,
    snapNote: null,
    status: '',
    cursor: null,
  };
}

/**
 * A session plus the one piece of hidden state a pure record cannot hold: the
 * hold clock. `SnapHold` is an object with behaviour, so it is kept beside the
 * state rather than inside it — which also keeps the state serialisable, so tests
 * can compare two of them with a deep equality.
 */
export class DrawSession {
  state: DrawSessionState;
  readonly hold = new SnapHold();

  constructor(tool: ToolId = 'select') {
    this.state = initialSession(tool);
  }

  /**
   * Apply a patch and return the new state.
   *
   * A tiny helper rather than a setter per field: the JSX re-renders from
   * `state`, and one `mutate({ … })` call per engine reply keeps the mapping from
   * "what the engine said" to "what is on screen" readable in one place.
   */
  mutate(patch: Partial<DrawSessionState>): DrawSessionState {
    this.state = { ...this.state, ...patch };
    return this.state;
  }

  /** Switch tools. A half-finished gesture is cancelled by the caller, not here:
   *  cancelling it needs the engine, and this class never talks to the engine. */
  setTool(tool: ToolId): DrawSessionState {
    return this.mutate({ tool });
  }

  get isPen(): boolean {
    return this.state.tool === 'pen';
  }

  get isBrush(): boolean {
    return this.state.tool === 'brush';
  }

  get isDirect(): boolean {
    return this.state.tool === 'direct';
  }

  /** The engine's two-tool drawing vocabulary, for the active tool. */
  get drawTool(): DrawTool | null {
    if (this.state.tool === 'pen') return 'pen';
    if (this.state.tool === 'brush') return 'brush';
    return null;
  }

  /** Open a gesture from a press. */
  press(press: {
    tool: DrawTool;
    pointerId: number;
    point: Point;
    client: Point;
    alt: boolean;
    pressure: number | null;
  }): DrawSessionState {
    this.hold.reset();
    return this.mutate({
      isDrawing: true,
      committed: false,
      isAltPressed: press.alt,
      gesture: beginGesture(press),
      draft: null,
      samples: null,
      snapNote: null,
      cursor: press.point,
    });
  }

  /** Extend the live gesture with a move. */
  extend(point: Point, client: Point, pressure: number | null): DrawSessionState {
    const gesture = this.state.gesture;
    if (!gesture) return this.state;
    return this.mutate({ gesture: extendGesture(gesture, point, client, pressure), cursor: point });
  }

  /** Close the gesture (the engine has been told; the draft stays until the reply). */
  release(): DrawSessionState {
    const gesture = this.state.gesture;
    if (!gesture) return this.state;
    this.hold.reset();
    return this.mutate({ isDrawing: false, gesture: null, isHoldingForSnap: false });
  }

  /** Fold a `draw_pointer` reply in: the draft, the samples, the snap verdict. */
  absorbReply(reply: DrawReplyWire): DrawSessionState {
    if (!reply.ok) {
      return this.mutate({
        status: reply.error ? `✗ ${reply.error}` : '✗ the engine refused the gesture',
      });
    }
    this.hold.observe(reply.snap_candidate ?? false);
    const patch: Partial<DrawSessionState> = {
      draft: reply.draft ?? null,
      samples: reply.samples ?? null,
    };
    if (reply.draft) {
      patch.status = draftStatus(reply.draft.kinds.length, reply.draft.snap_candidate);
    }
    return this.mutate(patch);
  }

  /** Fold a commit in: the engine named a node, and the session now owns it. */
  absorbCommit(reply: DrawReplyWire): DrawSessionState {
    this.hold.reset();
    const patch: Partial<DrawSessionState> = {
      isDrawing: false,
      committed: false,
      gesture: null,
      draft: null,
      samples: null,
      isHoldingForSnap: false,
    };
    if (!reply.ok) {
      patch.status = reply.error ? `✗ ${reply.error}` : '✗ the engine refused the commit';
      return this.mutate(patch);
    }
    if (reply.node_id) patch.nodeId = reply.node_id;
    if (reply.snapped) {
      patch.snapNote = `snapped a ${reply.snapped}${
        typeof reply.snap_error === 'number' ? ` (±${reply.snap_error.toFixed(1)} units)` : ''
      }`;
    }
    const segments = typeof reply.segments === 'number' ? reply.segments : null;
    patch.status = reply.snapped
      ? `✎ ${patch.snapNote}`
      : segments !== null
        ? `✎ path committed · ${segments} segment${segments === 1 ? '' : 's'}${
            typeof reply.fit_error === 'number' ? ` · fit ±${reply.fit_error.toFixed(2)}` : ''
          }`
        : '✎ the engine committed the path';
    return this.mutate(patch);
  }

  /** Fold an overlay read in (after a tool change, a selection, or an undo). */
  absorbOverlay(nodeId: string | null, anchors: AnchorWire[]): DrawSessionState {
    const selected =
      this.state.selectedSlot && anchors.some((a) => a.slot === this.state.selectedSlot)
        ? this.state.selectedSlot
        : null;
    return this.mutate({ nodeId, anchors, selectedSlot: selected });
  }

  /** Where the white arrow's click landed: a point, the whole path, or nothing. */
  select(hit: Hit): DrawSessionState {
    if (hit.kind === 'anchor') {
      return this.mutate({ selectedSlot: hit.slot, status: `◧ ${hit.slot} selected` });
    }
    if (hit.kind === 'handle') {
      return this.mutate({ selectedSlot: hit.slot, hoveredSlot: hit.slot });
    }
    return this.mutate({ selectedSlot: null, status: '◧ the whole path is selected' });
  }

  /** The hold clock matured (called by the timer the session schedules). */
  firedHold(): DrawSessionState {
    return this.mutate({ isHoldingForSnap: false, status: '✦ snapping…' });
  }

  /** Nothing to snap: the stroke never looked like a primitive. */
  refuseSnap(message: string): DrawSessionState {
    this.hold.reset();
    return this.mutate({ isHoldingForSnap: false, snapNote: message, status: `✗ ${message}` });
  }
}

/** The one-line status a draft earns: how much is drawn, and whether it could snap. */
export function draftStatus(segments: number, snapCandidate: boolean): string {
  if (segments === 0 && !snapCandidate) return 'drawing — click to place the first point';
  const shape = `${segments} segment${segments === 1 ? '' : 's'}`;
  return snapCandidate ? `✎ ${shape} · hold to snap` : `✎ ${shape} · Enter to finish`;
}

/** The cursor name for the active tool and state — RULE 4's "show the tool cursor". */
export function pointerCursor(tool: ToolId, isDrawing: boolean): string {
  if (isDrawing) return 'grabbing';
  switch (tool) {
    case 'pen':
      return 'crosshair';
    case 'brush':
      return 'crosshair';
    case 'direct':
      return 'pointer';
    default:
      return 'default';
  }
}
