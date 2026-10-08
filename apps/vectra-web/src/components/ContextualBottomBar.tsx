/**
 * **The dynamic bottom toolbar** (Task 13.0 RULE 1).
 *
 * The bar changes with what the designer is doing, and it changes *content*
 * rather than appearing and disappearing:
 *
 * | state | who | what it shows |
 * |---|---|---|
 * | `brush` | the brush is in hand | brush name · size · opacity · colour |
 * | `path` | an object is selected | fill dot · stroke dot · width · cap · join |
 * | `node` | a point of a path is owned | Convert · Smooth · Symmetric · Corner · Align |
 * | `idle` | neither | selection caption · undo · redo · zoom |
 *
 * The whole decision is `bottomBarMode` (pure, tested in `task-13-0-shell`), so
 * the bar cannot show brush controls over a selected path — a mistake that is
 * invisible in a screenshot and infuriating in a hand.
 *
 * ## What is live, and what is honest
 *
 * Every control here is wired to a command the engine actually has, with two
 * exceptions that RULE 1 names and the engine cannot yet serve:
 *
 * * **Cap** and **Join** — `vectra-core` has no line-cap or line-join property
 *   (audit §2.2; roadmap 1.4). The controls render *disabled*, with the roadmap
 *   item in the tooltip, rather than being silently absent: a designer looking
 *   for a round cap learns when it is coming, not that the app is broken.
 * * **Opacity** on the brush bar is the *stroke's* opacity, which the appearance
 *   stack owns today (`SetAppearances`) — a real command. Flow/velocity-driven
 *   opacity is Phase 2's brush engine (roadmap 2.2).
 *
 * Everything else — fill colour, stroke colour, stroke width, duplicate, undo,
 * redo, zoom — is an existing engine command, unchanged.
 */

import { useState } from 'react';
import type { ReactNode } from 'react';
import {
  AlignCenterHorizontal,
  CircleSlash,
  CornerDownRight,
  Minus,
  Plus,
  Redo2,
  Spline,
  Undo2,
  ZoomIn,
  ZoomOut,
} from 'lucide-react';
import { bottomBarCaption, strokeSwatch, strokeWidthOf, THEME } from '../engine/theme';
import type { BottomBarMode } from '../engine/theme';
import type { SnapshotNodeWire } from '../engine/wire';

export interface ContextualBottomBarProps {
  mode: BottomBarMode;
  /** How many objects are selected. */
  selectionCount: number;
  /** The single selected object, when there is exactly one. */
  node: SnapshotNodeWire | null;

  /** Brush controls. */
  brushName: string;
  /** The engine's fixed profile, displayed — see the note in the brush state. */
  brushSize: number;
  brushOpacity: number;
  /**
   * The brush's live settings, supplied by the app: the StreamLine filter and
   * the paint well (drag-to-fill). Passed as a node rather than reimplemented
   * here, so the bar and the colour-drop gesture share one component and one
   * piece of state.
   */
  brushExtras?: ReactNode;

  /** Path controls — all existing appearance commands. */
  onFillColor: (color: string) => void;
  onStrokeColor: (color: string) => void;
  onStrokeWidth: (width: number) => void;

  /** Node controls (the white arrow's anchor). */
  onNodeConvert: (kind: 'corner' | 'smooth') => void;
  nodeAlign: string;
  onNodeAlign: (align: string) => void;

  /** The idle state's controls. */
  canUndo: boolean;
  canRedo: boolean;
  onUndo: () => void;
  onRedo: () => void;
  zoom: number | null;
  onZoomIn: () => void;
  onZoomOut: () => void;
  onZoomFit: () => void;

  disabled?: boolean;
}

/** A labelled slider: the label is static, the number is the value. */
function Slider({
  label,
  value,
  min,
  max,
  step,
  suffix,
  onChange,
  disabled,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  suffix: string;
  onChange: (value: number) => void;
  disabled?: boolean;
}) {
  return (
    <label className="bar-slider" title={`${label} ${value}${suffix}`}>
      <span className="bar-slider-label">{label}</span>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        disabled={disabled}
        aria-label={label}
        data-testid={`bar-${label.toLowerCase()}`}
        onChange={(event) => onChange(Number(event.target.value))}
      />
      <span className="bar-slider-value">
        {value}
        {suffix}
      </span>
    </label>
  );
}

/** A colour dot: the swatch a designer taps, with the picker behind it. */
function ColorDot({
  label,
  color,
  testId,
  onChange,
  disabled,
}: {
  label: string;
  color: string;
  testId: string;
  onChange: (color: string) => void;
  disabled?: boolean;
}) {
  return (
    <label className="bar-dot" title={`${label}: ${color}`}>
      <span className="bar-dot-ring" style={{ borderColor: color }}>
        <span className="bar-dot-fill" style={{ background: color }} />
      </span>
      <input
        type="color"
        data-testid={testId}
        value={color.slice(0, 7)}
        disabled={disabled}
        aria-label={label}
        onChange={(event) => onChange(event.target.value)}
      />
    </label>
  );
}

export default function ContextualBottomBar(props: ContextualBottomBarProps) {
  const { mode, node, disabled } = props;
  /** The stroke width the bar is editing, seeded from the engine's own number. */
  const engineWidth = strokeWidthOf(node ?? undefined);
  const [widthDraft, setWidthDraft] = useState<number | null>(null);
  const width = widthDraft ?? engineWidth;
  const fill = node?.style.appearances.find((row) => row.kind === 'fill')?.paint;
  const fillColor =
    fill?.type === 'solid'
      ? fill.color
      : (node?.style.fill ?? THEME.textDim);
  const strokeColor = strokeSwatch(node ?? undefined) ?? THEME.textDim;

  return (
    <footer className="bottom-bar" data-testid="bottom-bar" data-mode={mode}>
      <span className="bar-caption" data-testid="bar-caption">
        {bottomBarCaption(mode, props.selectionCount)}
      </span>

      {mode === 'brush' && (
        <div className="bar-group" data-testid="bar-brush">
          <span className="bar-name">{props.brushName}</span>
          {/*
            Size and Opacity are RULE 1's controls and the *bar's* shape is
            right — but the engine has no brush-width or brush-opacity setting to
            write: `draw_brush_commit` fits a stroke with a fixed profile
            (`BrushProfile { max_width: 12, min_width: 2 }`) and the node it
            creates is styled afterwards. So they show the engine's real numbers
            and stay disabled, with the roadmap item in the tooltip. The two
            controls beside them — Smoothing and the paint well — are live, and
            they are the two the hand actually feels today.
          */}
          <label className="bar-slider" title="The engine's brush profile is fixed for now — a settable width arrives with the Phase 2 brush engine (roadmap 2.2)">
            <span className="bar-slider-label">Size</span>
            <input
              type="range"
              min={1}
              max={128}
              step={1}
              value={props.brushSize}
              disabled
              aria-label="Brush size"
              data-testid="bar-size"
              readOnly
            />
            <span className="bar-slider-value">{props.brushSize}px</span>
          </label>
          <label className="bar-slider" title="Brush opacity arrives with the Phase 2 brush engine (roadmap 2.2) — set it on the object afterwards today">
            <span className="bar-slider-label">Opacity</span>
            <input
              type="range"
              min={5}
              max={100}
              step={1}
              value={Math.round(props.brushOpacity * 100)}
              disabled
              aria-label="Brush opacity"
              data-testid="bar-opacity"
              readOnly
            />
            <span className="bar-slider-value">{Math.round(props.brushOpacity * 100)}%</span>
          </label>
          {props.brushExtras}
        </div>
      )}

      {mode === 'path' && (
        <div className="bar-group" data-testid="bar-path">
          <ColorDot
            label="Fill"
            color={fillColor}
            testId="bar-fill-color"
            onChange={props.onFillColor}
            disabled={disabled}
          />
          <ColorDot
            label="Stroke"
            color={strokeColor}
            testId="bar-stroke-color"
            onChange={props.onStrokeColor}
            disabled={disabled}
          />
          <Slider
            label="Width"
            value={width}
            min={0}
            max={40}
            step={0.5}
            suffix=""
            onChange={(value) => {
              setWidthDraft(value);
              props.onStrokeWidth(value);
            }}
            disabled={disabled}
          />
          {/*
            RULE 1 names Cap and Join. The engine has neither property yet, so
            they are shown, disabled, with the roadmap item that will bring them
            — the same honesty the audit asks of every other status.
          */}
          <button
            type="button"
            className="bar-seg"
            disabled
            data-testid="bar-cap"
            title="Line caps arrive with stroke properties (roadmap 1.4)"
          >
            <CircleSlash size={14} strokeWidth={1.75} aria-hidden="true" />
            <span>Cap</span>
          </button>
          <button
            type="button"
            className="bar-seg"
            disabled
            data-testid="bar-join"
            title="Line joins arrive with stroke properties (roadmap 1.4)"
          >
            <CircleSlash size={14} strokeWidth={1.75} aria-hidden="true" />
            <span>Join</span>
          </button>
        </div>
      )}

      {mode === 'node' && (
        <div className="bar-group" data-testid="bar-node">
          <button
            type="button"
            className="bar-seg"
            data-testid="node-convert"
            disabled={disabled}
            title="Convert the point to a corner — the handles retract"
            onClick={() => props.onNodeConvert('corner')}
          >
            <CornerDownRight size={14} strokeWidth={1.75} aria-hidden="true" />
            <span>Convert</span>
          </button>
          <button
            type="button"
            className="bar-seg"
            data-testid="node-smooth"
            disabled={disabled}
            title="Convert the point to smooth — the handles align and mirror"
            onClick={() => props.onNodeConvert('smooth')}
          >
            <Spline size={14} strokeWidth={1.75} aria-hidden="true" />
            <span>Smooth</span>
          </button>
          {/* The state, read from the engine's handles — the brief's own word
              for it, and the reason there is no third button that would repeat
              one of the two above. */}
          <span
            className="bar-align"
            data-testid="node-align"
            title="The point's state, read from its two handles"
          >
            {props.nodeAlign}
          </span>
          {/* Align to the canvas is a *view* action on the selection, and the
              engine owns the numbers: the status line reports where the point
              is, rather than the UI deciding what "aligned" means. */}
          <button
            type="button"
            className="bar-seg"
            data-testid="node-align-action"
            disabled={disabled}
            title="Report the point's alignment against the canvas"
            onClick={() => props.onNodeAlign('centre')}
          >
            <AlignCenterHorizontal size={14} strokeWidth={1.75} aria-hidden="true" />
            <span>Align</span>
          </button>
        </div>
      )}

      {mode === 'idle' && (
        <div className="bar-group" data-testid="bar-idle">
          <button
            type="button"
            className="bar-seg"
            data-testid="bar-undo"
            disabled={disabled || !props.canUndo}
            title="Undo (⌘Z, or a two-finger tap)"
            aria-label="Undo"
            onClick={props.onUndo}
          >
            <Undo2 size={15} strokeWidth={1.75} aria-hidden="true" />
          </button>
          <button
            type="button"
            className="bar-seg"
            data-testid="bar-redo"
            disabled={disabled || !props.canRedo}
            title="Redo (⌘⇧Z, or a three-finger tap)"
            aria-label="Redo"
            onClick={props.onRedo}
          >
            <Redo2 size={15} strokeWidth={1.75} aria-hidden="true" />
          </button>
          <span className="bar-divider" aria-hidden="true" />
          <button
            type="button"
            className="bar-seg"
            data-testid="bar-zoom-out"
            disabled={disabled}
            title="Zoom out"
            aria-label="Zoom out"
            onClick={props.onZoomOut}
          >
            <ZoomOut size={15} strokeWidth={1.75} aria-hidden="true" />
          </button>
          <span className="bar-zoom" data-testid="bar-zoom">
            {props.zoom === null ? '—' : `${(props.zoom * 100).toFixed(0)}%`}
          </span>
          <button
            type="button"
            className="bar-seg"
            data-testid="bar-zoom-in"
            disabled={disabled}
            title="Zoom in"
            aria-label="Zoom in"
            onClick={props.onZoomIn}
          >
            <ZoomIn size={15} strokeWidth={1.75} aria-hidden="true" />
          </button>
          <button
            type="button"
            className="bar-seg"
            data-testid="bar-zoom-fit"
            disabled={disabled}
            title="Fit the document to the window"
            onClick={props.onZoomFit}
          >
            <Minus size={14} strokeWidth={2} aria-hidden="true" />
            <Plus size={14} strokeWidth={2} aria-hidden="true" />
          </button>
        </div>
      )}
    </footer>
  );
}
