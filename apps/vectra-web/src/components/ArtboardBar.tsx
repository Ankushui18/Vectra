/**
 * **The Artboard bar** (Task 10.2 RULE 2) — the dropdown that jumps between
 * canvases, and the two export buttons the rule asks for.
 *
 * ```text
 *   Artboards ▾  Logo 1024×768      [+ board]  [Export current] [Export all]
 * ```
 *
 * Jumping is two facts, and both come from the engine: `SetActiveArtboard` says
 * *which* board the designer is on (so new artwork and "export current" mean the
 * same board), and the board's own bounds say *where* it is. Framing the camera
 * on those bounds is the renderer's job — the panel hands it a rectangle, it
 * does not compute a projection.
 *
 * The export buttons call the engine's own exporters, which read the *scene*,
 * not the panel's idea of it.
 */

import {
  setArtboardBackground,
  setArtboardBounds,
  createArtboard,
  deleteArtboard,
  renameArtboard,
  setActiveArtboard,
} from '../engine/commands';
import { useState } from 'react';
import { artboardFrame, artboardRows, hexToColor } from '../engine/panels';
import type { ArtboardRow } from '../engine/panels';
import type { CommandWire, SnapshotWire } from '../engine/wire';

export interface ArtboardBarProps {
  snapshot: SnapshotWire | null;
  onCommand: (label: string, command: CommandWire) => void;
  /** Frame the canvas on a document rectangle (the camera's, not the panel's). */
  onFrame: (rect: { x: number; y: number; width: number; height: number }) => void;
  onExportCurrent: () => void;
  onExportAll: () => void;
  disabled?: boolean;
}

export function ArtboardBar({
  snapshot,
  onCommand,
  onFrame,
  onExportCurrent,
  onExportAll,
  disabled,
}: ArtboardBarProps) {
  const boards = artboardRows(snapshot);
  const active = boards.find((board) => board.active) ?? boards[0] ?? null;
  // The box and the colour are *edits in progress*, so they are local until
  // committed — a half-typed `12` in the width field must not resize the board
  // under the designer's hand, and must not enter the undo history either.
  const [draft, setDraft] = useState<BoxDraft | null>(null);
  const base = draft && draft.id === active?.id ? draft : draftFor(active);
  const shown: BoxDraft | null = base;

  /** The draft with one field replaced — the only place a field is edited. */
  const edit = (field: keyof Omit<BoxDraft, 'id'>, value: string) => {
    if (!shown) return;
    setDraft({ ...shown, [field]: value });
  };

  const commitBox = () => {
    if (!active || !shown) return;
    const next = boxFromDraft(shown);
    if (next) {
      const changed =
        next.x !== active.bounds[0] ||
        next.y !== active.bounds[1] ||
        next.width !== active.bounds[2] ||
        next.height !== active.bounds[3];
      if (changed) onCommand('Frame artboard', setArtboardBounds(active.id, next));
    }
    setDraft(null);
  };

  const commitBackground = () => {
    if (!active || !shown) return;
    const hex = shown.background.trim();
    if (isHexColor(hex)) {
      // Normalised before comparing, so re-typing `#FFFFFF` over `#ffffff` is
      // not a command and not an undo entry.
      const normalised = `#${hex.replace('#', '').toLowerCase()}`;
      if (normalised !== active.background.toLowerCase()) {
        onCommand('Artboard background', setArtboardBackground(active.id, hexToColor(hex)));
      }
    }
    setDraft(null);
  };

  return (
    <section className="panel artboard-bar" data-testid="artboard-bar">
      <h2>
        Artboards
        <span className="sub">
          {boards.length} {boards.length === 1 ? 'canvas' : 'canvases'} · jump, frame, export
        </span>
      </h2>
      <div className="artboard-row">
        <label className="artboard-select">
          <span className="sr-only">Active artboard</span>
          <select
            data-testid="artboard-select"
            disabled={disabled || boards.length === 0}
            value={active?.id ?? ''}
            onChange={(event) => {
              const board = boards.find((candidate) => candidate.id === event.target.value);
              if (!board) return;
              onCommand('Active artboard', setActiveArtboard(board.id));
              const frame = artboardFrame(board);
              if (frame) onFrame(frame);
            }}
          >
            {boards.length === 0 && <option value="">No artboards</option>}
            {boards.map((board) => (
              <option key={board.id} value={board.id}>
                {board.name} · {board.label}
              </option>
            ))}
          </select>
        </label>

        <button
          className="mini"
          data-testid="artboard-frame"
          disabled={disabled || !active}
          title="Frame the canvas on this artboard"
          onClick={() => {
            const frame = artboardFrame(active);
            if (frame) onFrame(frame);
          }}
        >
          ⤢ fit
        </button>

        <button
          className="mini"
          data-testid="artboard-add"
          disabled={disabled}
          title="New artboard next to this one"
          onClick={() => {
            // The new board is placed to the right of the one the designer is
            // on, and *framed*: `CreateArtboard` makes it the active board (the
            // engine's own rule — a create is a move), so the canvas follows the
            // designer onto the paper they just asked for.
            const [x, y, width, height] = active?.bounds ?? [0, 0, 800, 600];
            const rect = { x: x + width + 40, y, width, height };
            onCommand(
              'Create artboard',
              createArtboard({
                name: `Artboard ${boards.length + 1}`,
                ...rect,
                background: { r: 255, g: 255, b: 255, a: 255 },
              }),
            );
            onFrame(rect);
          }}
        >
          + board
        </button>

        <span className="artboard-actions">
          <button
            data-testid="export-current-artboard"
            disabled={disabled}
            onClick={onExportCurrent}
          >
            Export current artboard
          </button>
          <button data-testid="export-all-artboards" disabled={disabled} onClick={onExportAll}>
            Export all artboards
          </button>
        </span>
      </div>

      {active && (
        <div className="artboard-detail" data-testid="artboard-detail">
          <span className="chip" style={{ background: active.background }}>
            &nbsp;
          </span>
          <button
            className="link"
            data-testid="artboard-rename"
            disabled={disabled}
            onClick={() => {
              const name = window.prompt('Artboard name', active.name);
              if (name && name.trim()) {
                onCommand('Rename artboard', renameArtboard(active.id, name.trim()));
              }
            }}
          >
            {active.name}
          </button>
          <span className="sub">
            {active.label} · {active.layers} {active.layers === 1 ? 'layer' : 'layers'}
          </span>
          <button
            className="icon danger"
            data-testid="artboard-delete"
            disabled={disabled || boards.length <= 1}
            title={
              boards.length <= 1
                ? 'A document keeps at least one artboard'
                : 'Delete this artboard (its layers survive)'
            }
            onClick={() => onCommand('Delete artboard', deleteArtboard(active.id))}
          >
            ✕
          </button>

          {/* RULE 2's other half: the board owns its box and its colour, and both
              are editable here. Four numbers and one hex string — the engine
              holds the truth, this row holds what is being typed. */}
          <span className="artboard-box" data-testid="artboard-box">
            {(['x', 'y', 'width', 'height'] as const).map((field) => (
              <label key={field} className="board-field">
                <span className="sr-only">{field}</span>
                <input
                  className="board-input"
                  data-testid={`artboard-${field}`}
                  type="number"
                  inputMode="decimal"
                  disabled={disabled}
                  value={String(shown?.[field] ?? '')}
                  onChange={(event) => edit(field, event.target.value)}
                  onBlur={commitBox}
                  onKeyDown={(event) => {
                    if (event.key === 'Enter') commitBox();
                    if (event.key === 'Escape') setDraft(null);
                  }}
                />
              </label>
            ))}
            <label className="board-field">
              <span className="sr-only">background</span>
              <input
                className="board-input board-hex"
                data-testid="artboard-background"
                type="text"
                spellCheck={false}
                disabled={disabled}
                value={String(shown?.background ?? '')}
                onChange={(event) => edit('background', event.target.value)}
                onBlur={commitBackground}
                onKeyDown={(event) => {
                  if (event.key === 'Enter') commitBackground();
                  if (event.key === 'Escape') setDraft(null);
                }}
              />
            </label>
          </span>
        </div>
      )}
    </section>
  );
}

export default ArtboardBar;

/** The four numbers the box editor edits, as strings (a field is text while it
 *  is being typed, and `''` is a state a number input can genuinely be in). */
interface BoxDraft {
  id: string;
  x: string;
  y: string;
  width: string;
  height: string;
  background: string;
}

/** The draft a board starts as: its own numbers, nothing invented. */
function draftFor(board: ArtboardRow | null): BoxDraft | null {
  if (!board) return null;
  return {
    id: board.id,
    x: String(board.bounds[0]),
    y: String(board.bounds[1]),
    width: String(board.bounds[2]),
    height: String(board.bounds[3]),
    // The snapshot sends the board's colour as `#rrggbb[aa]` (the engine's own
    // `Color::to_hex`), which is exactly what the field edits and the swatch
    // renders — no conversion, so no chance of one.
    background: board.background,
  };
}

/**
 * A committed draft, or `null` if it is not yet a box.
 *
 * A board with no width is not a small board, it is a missing one, so an empty
 * or non-positive size is refused here — before it reaches the engine, where it
 * would be a document a designer cannot see. The engine's own validation stays
 * the authority; this only stops the keystroke *between* two digits from
 * becoming a command.
 */
function boxFromDraft(draft: BoxDraft): { x: number; y: number; width: number; height: number } | null {
  const x = Number(draft.x);
  const y = Number(draft.y);
  const width = Number(draft.width);
  const height = Number(draft.height);
  if (![x, y, width, height].every((value) => Number.isFinite(value))) return null;
  if (width <= 0 || height <= 0) return null;
  return { x, y, width, height };
}

/** `#rrggbb` or `#rrggbbaa` — the two shapes the engine's colour takes. */
function isHexColor(text: string): boolean {
  return /^#?([0-9a-f]{6})([0-9a-f]{2})?$/i.test(text.trim());
}
