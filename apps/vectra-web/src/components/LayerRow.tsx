/**
 * **One layer row** (Task 13.0 RULE 2).
 *
 * RULE 2 asks the Layers panel to distinguish art object types *at a glance*:
 * `◉` paint, `◇` vector, `▣` group, `⌁` adjustment/mask, `T` text. This
 * component is where that glyph lives, and the reason it is its own file is that
 * a row is the densest thing in the UI: a disclosure triangle, a type, a name,
 * an active chip, a hidden chip, a rename field, and up to five row actions —
 * all in 22 pixels of height. Keeping it separate is what stops the panel's
 * drag-and-drop logic from growing a second job.
 *
 * Three rules are visible in the code below:
 *
 * * **The type comes from the wire, never the name.** `layerTypeOf` reads the
 *   snapshot; a row called "text" that is a rectangle wears `◇`. A type
 *   indicator that trusts a name is decoration, not information.
 * * **Every control is an icon** (RULE 5). The eye, the padlock, the alpha lock,
 *   the clip, rename and delete are `lucide-react` glyphs with their words in
 *   `title`/`aria-label` — a designer learns them once and never reads a row
 *   again.
 * * **The name is editable in place.** Double-click (or the pencil) turns the
 *   row into an input; `Enter` commits, `Escape` abandons, and both are one
 *   command, not a live-binding per keystroke.
 */

import type { DragEvent } from 'react';
import {
  ChevronDown,
  ChevronRight,
  Eye,
  EyeOff,
  Lock,
  LockOpen,
  Pencil,
  Trash2,
} from 'lucide-react';
import { LAYER_TYPES, layerMark } from '../engine/theme';
import type { LayerRow as LayerRowModel } from '../engine/panels';

export interface LayerRowProps {
  row: LayerRowModel;
  /** Is this row's contents listed? (A layer or group with a triangle.) */
  expanded: boolean;
  /** How many children the owning layer has (the triangle's disabled state). */
  childCount: number;
  /** The row currently being renamed, if any. */
  renaming: boolean;
  /** The draft name while renaming. */
  draftName: string;
  /** The row a drag is currently hovering. */
  dropTarget: boolean;
  /** A node is selected and waiting to be assigned to a layer. */
  assigning: boolean;
  /** The panel is inert (engine not ready). */
  disabled?: boolean;

  onToggleExpanded: () => void;
  onStartRename: () => void;
  onDraftName: (name: string) => void;
  onCommitRename: () => void;
  onCancelRename: () => void;

  /** The row was clicked (select / make active). */
  onSelect: () => void;
  /** Drag lifecycle for reordering and reparenting. */
  onDragStart: () => void;
  onDragEnd: () => void;
  onDragOver: (event: DragEvent) => void;
  onDragLeave: () => void;
  onDrop: (event: DragEvent) => void;

  /** Layer-only commands. */
  onToggleVisible?: () => void;
  onToggleLocked?: () => void;
  onToggleAlpha?: () => void;
  onToggleClip?: () => void;
  onDelete?: () => void;
}

export function LayerRow({
  row,
  expanded,
  childCount,
  renaming,
  draftName,
  dropTarget,
  assigning,
  disabled,
  onToggleExpanded,
  onStartRename,
  onDraftName,
  onCommitRename,
  onCancelRename,
  onSelect,
  onDragStart,
  onDragEnd,
  onDragOver,
  onDragLeave,
  onDrop,
  onToggleVisible,
  onToggleLocked,
  onToggleAlpha,
  onToggleClip,
  onDelete,
}: LayerRowProps) {
  const isLayer = row.kind === 'layer';
  // **RULE 2's indicator.** A group node says so; a layer that clips or locks its
  // alpha is a mask; everything else is a vector object until the engine has a
  // raster or an adjustment node to report.
  const type = layerMark(
    { kind: row.kind, isGroup: row.isGroup },
    isLayer
      ? { alpha_locked: row.alphaLocked === true, clipping_mask: row.clippingMask === true }
      : undefined,
  );
  const disclosure = isLayer || row.canOpen;

  return (
    <li
      className={[
        'layer-row',
        `layer-${row.kind}`,
        row.active ? 'active' : '',
        row.visible ? '' : 'hidden',
        row.locked ? 'locked' : '',
        dropTarget ? 'drop-target' : '',
        assigning ? 'assigning' : '',
      ]
        .filter(Boolean)
        .join(' ')}
      style={{ paddingLeft: 6 + row.depth * 16 }}
      data-testid={`layer-row-${row.id}`}
      draggable={!disabled}
      onDragStart={onDragStart}
      onDragEnd={onDragEnd}
      onDragOver={onDragOver}
      onDragLeave={onDragLeave}
      onDrop={onDrop}
      onClick={onSelect}
    >
      {disclosure ? (
        <button
          type="button"
          className="disclose"
          data-testid={isLayer ? `layer-expand-${row.id}` : `group-expand-${row.id}`}
          disabled={isLayer ? childCount === 0 : !row.canOpen}
          onClick={(event) => {
            event.stopPropagation();
            onToggleExpanded();
          }}
          title={`${expanded ? 'Collapse' : 'Expand'} ${row.name}`}
          aria-label={`${expanded ? 'Collapse' : 'Expand'} ${row.name}`}
          aria-expanded={expanded}
        >
          {expanded ? (
            <ChevronDown size={13} strokeWidth={2} aria-hidden="true" />
          ) : (
            <ChevronRight size={13} strokeWidth={2} aria-hidden="true" />
          )}
        </button>
      ) : (
        <span className="disclose-spacer" aria-hidden="true" />
      )}

      {/*
        The indicator itself, with its own testid namespace: a layer row's
        marker answers "what is this container" and a node row's answers "what
        is this object". Keeping them distinct matters because the row's own
        `layer-row-{id}` testid is how the panel's tests count *rows* — a marker
        that reused it would make one collapsed group look like two rows.
      */}
      <span
        className={`type-mark type-${type.type}`}
        data-testid={isLayer ? `layer-kind-${row.id}` : `layer-object-${row.id}`}
        title={type.label}
        aria-label={type.label}
        role="img"
      >
        {type.glyph}
      </span>

      {renaming ? (
        <input
          className="rename"
          autoFocus
          value={draftName}
          onChange={(event) => onDraftName(event.target.value)}
          onClick={(event) => event.stopPropagation()}
          onBlur={onCommitRename}
          onKeyDown={(event) => {
            if (event.key === 'Enter') onCommitRename();
            if (event.key === 'Escape') onCancelRename();
          }}
        />
      ) : (
        <span className="layer-name" onDoubleClick={onStartRename}>
          {row.name}
        </span>
      )}

      {isLayer && row.active && (
        <span className="chip chip-active" title="New artwork lands here">
          active
        </span>
      )}
      {!isLayer && row.ownVisible === false && (
        <span className="chip" title="Hidden by the node's own eye">
          hidden
        </span>
      )}

      <span className="layer-actions">
        {isLayer ? (
          <>
            <button
              type="button"
              className="icon"
              data-testid={`layer-eye-${row.id}`}
              disabled={disabled}
              title={row.visible ? 'Hide layer' : 'Show layer'}
              aria-label={`${row.visible ? 'Hide' : 'Show'} layer ${row.name}`}
              aria-pressed={!row.visible}
              onClick={(event) => {
                event.stopPropagation();
                onToggleVisible?.();
              }}
            >
              {row.visible ? (
                <Eye size={14} strokeWidth={1.75} aria-hidden="true" />
              ) : (
                <EyeOff size={14} strokeWidth={1.75} aria-hidden="true" />
              )}
            </button>
            <button
              type="button"
              className="icon"
              data-testid={`layer-lock-${row.id}`}
              disabled={disabled}
              title={row.locked ? 'Unlock layer' : 'Lock layer'}
              aria-label={`${row.locked ? 'Unlock' : 'Lock'} layer ${row.name}`}
              aria-pressed={row.locked}
              onClick={(event) => {
                event.stopPropagation();
                onToggleLocked?.();
              }}
            >
              {row.locked ? (
                <Lock size={14} strokeWidth={1.75} aria-hidden="true" />
              ) : (
                <LockOpen size={14} strokeWidth={1.75} aria-hidden="true" />
              )}
            </button>
            <button
              type="button"
              className="icon text-icon"
              data-testid={`layer-alpha-${row.id}`}
              disabled={disabled}
              title={
                row.alphaLocked
                  ? 'Unlock alpha — new strokes may leave the layer\u2019s artwork'
                  : 'Lock alpha — new strokes stay inside what this layer already holds'
              }
              aria-label={`${row.alphaLocked ? 'Unlock' : 'Lock'} alpha of layer ${row.name}`}
              aria-pressed={row.alphaLocked === true}
              onClick={(event) => {
                event.stopPropagation();
                onToggleAlpha?.();
              }}
            >
              {row.alphaLocked ? 'α' : 'a'}
            </button>
            <button
              type="button"
              className="icon text-icon"
              data-testid={`layer-clip-${row.id}`}
              // The bottom layer has nothing below it to clip to: the toggle is
              // disabled rather than allowed to mean "everything disappears".
              disabled={disabled || !row.clippedTo}
              title={
                !row.clippedTo
                  ? 'Nothing below this layer to clip to'
                  : row.clippingMask
                    ? 'Remove clipping mask — show this layer everywhere'
                    : 'Clip to layer below — show this layer only over the layer beneath'
              }
              aria-label={`${row.clippingMask ? 'Remove clipping mask from' : 'Clip'} layer ${row.name}`}
              aria-pressed={row.clippingMask === true}
              onClick={(event) => {
                event.stopPropagation();
                onToggleClip?.();
              }}
            >
              ▤
            </button>
            <button
              type="button"
              className="icon"
              data-testid={`layer-rename-${row.id}`}
              disabled={disabled}
              title="Rename layer"
              aria-label={`Rename layer ${row.name}`}
              onClick={(event) => {
                event.stopPropagation();
                onStartRename();
              }}
            >
              <Pencil size={13} strokeWidth={1.75} aria-hidden="true" />
            </button>
            <button
              type="button"
              className="icon danger"
              data-testid={`layer-delete-${row.id}`}
              disabled={disabled}
              title="Delete layer (its nodes survive, unassigned)"
              aria-label={`Delete layer ${row.name}`}
              onClick={(event) => {
                event.stopPropagation();
                onDelete?.();
              }}
            >
              <Trash2 size={13} strokeWidth={1.75} aria-hidden="true" />
            </button>
          </>
        ) : (
          <span className="layer-flags">
            {(row.ownVisible ?? true) ? (
              <Eye size={13} strokeWidth={1.75} aria-hidden="true" />
            ) : (
              <EyeOff size={13} strokeWidth={1.75} aria-hidden="true" />
            )}
            {row.ownLocked ? <Lock size={13} strokeWidth={1.75} aria-hidden="true" /> : null}
          </span>
        )}
      </span>
    </li>
  );
}

/** The glyph for a text node's row — exported so the panel's own legend can use it. */
export const TEXT_MARK = LAYER_TYPES.text.glyph;

export default LayerRow;
