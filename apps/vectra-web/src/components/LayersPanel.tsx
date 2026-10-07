/**
 * **The Layers Panel** (Task 10.2 RULE 1) — the right-hand, collapsible spine of
 * the workspace.
 *
 * ```text
 *   ┌ Layers ──────────────────────────────── + ─┬─┐
 *   │ ▾ Top            (active, 2 items)          │ │   drag the body to reorder
 *   │   🔒 ✎ Middle                                │ │   ▾/▸ opens a group
 *   │     ▣ Badge  ▾                               │ │   ◉ eye · 🔒 padlock
 *   │       ◆ Ring                                 │ │   ✎ renames in place
 *   │ ▸ Base                                       │ │
 *   └──────────────────────────────────────────────┘ │
 * ```
 *
 * **Two levels of it are a tree** (Task 10.3 RULE 1): a layer discloses its
 * contents, and a *group* among them discloses its own — the same clique of rows,
 * one indent deeper, driven by the same open-set. The rows come from
 * `engine/panels.ts` (`layerRows`), so which rows exist, in which order, at which
 * depth is a pure function that is unit-tested without a DOM; this file decides
 * only pixels, triangles and click targets.
 *
 * Every fact on screen comes from the snapshot: which layers exist, what they
 * are called, whether they are visible or locked, what they contain, which one
 * is active. The panel's only opinions are **presentation** ones — rows read
 * top-first (the way every design tool stacks them) and a collapsed layer hides
 * its contents.
 *
 * The two icons are the whole of RULE 4 from the UI's side: clicking one sends
 * an ordinary `SetLayerVisible` / `SetLayerLocked` command, and the engine turns
 * it into a presentation flag with no re-evaluation. The panel does not know
 * that, which is exactly why it cannot break it.
 */

import { useState } from 'react';
import type { DragEvent } from 'react';
import {
  createLayer,
  deleteLayer,
  renameLayer,
  reorderLayer,
  setActiveLayer,
  setLayerAlphaLocked,
  setLayerClippingMask,
  setLayerLocked,
  setLayerVisible,
  setNodeParent,
  moveNodeToLayer,
} from '../engine/commands';
import { dropIndexFor, dropNodeInto, isDescendantOf, layerOrder, layerRows } from '../engine/panels';
import type { LayerRow } from '../engine/panels';
import type { CommandWire, SnapshotWire } from '../engine/wire';

export interface LayersPanelProps {
  snapshot: SnapshotWire | null;
  /** The layer rows the user has opened (`collapsible` in RULE 1). */
  expanded: ReadonlySet<string>;
  onToggleExpanded: (layerId: string) => void;
  onCommand: (label: string, command: CommandWire) => void;
  /** Drop a node onto a layer row (RULE 1: "every node belongs to a Layer"). */
  onAssignNode?: (layerId: string) => void;
  /** **Group the selected nodes** (Task 10.4 RULE 1): the engine makes a group
   *  and moves the selection inside it, as one user action. */
  onGroupSelection?: () => void;
  /** The selected node ids, so the panel can offer the action when there is
   *  something to group (and dim it when there is not). */
  selection?: readonly string[];
  /** The pending node assignment, if a node is selected and waiting for a layer. */
  pendingNode?: string | null;
  disabled?: boolean;
}

export function LayersPanel({
  snapshot,
  expanded,
  onToggleExpanded,
  onCommand,
  onAssignNode,
  onGroupSelection,
  selection,
  pendingNode,
  disabled,
}: LayersPanelProps) {
  const [collapsed, setCollapsed] = useState(false);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [draftName, setDraftName] = useState('');
  const [dragLayer, setDragLayer] = useState<string | null>(null);
  /** The row being dragged: layers reorder among themselves, nodes reparent. */
  const [dragRow, setDragRow] = useState<{ id: string; kind: 'layer' | 'node' } | null>(null);
  const [dropTarget, setDropTarget] = useState<string | null>(null);

  const layers = snapshot?.layers ?? [];
  const rows: LayerRow[] = layerRows(snapshot, expanded);

  if (collapsed) {
    return (
      <section className="panel layers-panel collapsed" data-testid="layers-panel">
        <h2>
          <button
            className="panel-toggle"
            data-testid="layers-toggle"
            onClick={() => setCollapsed(false)}
            title="Show layers"
          >
            ▸
          </button>
          Layers <span className="sub">{layers.length}</span>
        </h2>
      </section>
    );
  }

  const startRename = (row: LayerRow) => {
    setRenaming(row.id);
    setDraftName(row.name);
  };

  const commitRename = () => {
    if (!renaming) return;
    const row = rows.find((candidate) => candidate.id === renaming);
    const name = draftName.trim();
    if (row && name && name !== row.name) {
      onCommand(
        `Rename ${row.kind}`,
        row.kind === 'layer' ? renameLayer(row.id, name) : renameNodeCommand(row.id, name),
      );
    }
    setRenaming(null);
  };

  const onDropLayer = (targetId: string, event: DragEvent) => {
    event.preventDefault();
    const count = layers.length;
    const topFirst = layerOrder(snapshot);
    const targetIndex = topFirst.indexOf(targetId);
    if (dragLayer && targetIndex >= 0) {
      const index = dropIndexFor(targetIndex, count);
      if (dragLayer !== targetId) {
        onCommand('Reorder layer', reorderLayer(dragLayer, index));
      }
    }
    setDragLayer(null);
    setDropTarget(null);
  };

  /**
   * **Drop a dragged node** (Task 10.4 RULE 1).
   *
   * The decision — which container, which position — is `dropNodeAt`, a pure
   * function of the snapshot (tested without a DOM). What is left here is the
   * gesture's plumbing: refuse a drop that would put a group inside itself
   * (the engine would refuse it too, typed) and send the move.
   */
  const onDropNode = (
    target: { id: string; kind: 'layer' | 'node'; canOpen?: boolean; layerId: string },
    event: DragEvent,
  ) => {
    event.preventDefault();
    const dragged = dragRow?.id;
    setDragRow(null);
    setDropTarget(null);
    const owning = dragged ? layerFor(dragged) : null;
    if (!dragged || !owning) return;
    // The **target row's** layer decides where the node is going; the node's own
    // layer only decides whether this is a reorder or a move to another layer.
    const targetLayer = layers.find((candidate) => candidate.id === target.layerId) ?? null;
    if (!targetLayer || isDescendantOf(targetLayer, target.id, dragged)) return;
    const drop = dropNodeInto(layers, targetLayer.id, target, dragged);
    if (!drop) return;
    if (drop.layer === owning.id) {
      onCommand(
        drop.parent ? 'Move into group' : 'Move out of group',
        setNodeParent(dragged, drop.parent, drop.index),
      );
      return;
    }
    // Another layer: membership and parentage are two facts, so one batch — and
    // one undo, which is what a designer expects of a single drag.
    onCommand(
      'Move to layer',
      moveNodeToLayer(dragged, drop.layer, drop.parent, drop.index),
    );
  };

  /** The layer a node id belongs to, as the snapshot reports it. */
  const layerFor = (nodeId: string) =>
    layers.find((candidate) => candidate.children.includes(nodeId)) ?? null;

  return (
    <section className="panel layers-panel" data-testid="layers-panel">
      <h2>
        <button
          className="panel-toggle"
          data-testid="layers-toggle"
          onClick={() => setCollapsed(true)}
          title="Collapse layers"
        >
          ▾
        </button>
        Layers
        <span className="sub">
          {layers.length} {layers.length === 1 ? 'layer' : 'layers'} · back → front reading
          top-down
        </span>
        <button
          className="mini"
          data-testid="add-layer"
          disabled={disabled}
          title="New layer"
          onClick={() => onCommand('Create layer', createLayer(`Layer ${layers.length + 1}`))}
        >
          +
        </button>
        {onGroupSelection && (
          <button
            className="mini"
            data-testid="group-selection"
            disabled={disabled || (selection?.length ?? 0) === 0}
            title={
              (selection?.length ?? 0) === 0
                ? 'Select one or more nodes to group them'
                : `Group ${selection?.length} selected node${(selection?.length ?? 0) === 1 ? '' : 's'}`
            }
            aria-label="Group the selection"
            onClick={() => onGroupSelection()}
          >
            ▣
          </button>
        )}
      </h2>

      {layers.length === 0 && (
        <p className="hint" data-testid="layers-empty">
          No layers yet. A new document gets one the moment artwork lands in it — or press
          <b> +</b> to make one now. Every node belongs to a layer, and an unassigned node still
          draws (that is how a document made before layers existed keeps working).
        </p>
      )}

      <ul className="layer-list" data-testid="layer-list">
        {rows.map((row) => {
          const layer = layers.find((candidate) => candidate.id === row.layerId);
          const childCount = layer?.children.length ?? 0;
          const isRenaming = renaming === row.id;
          return (
            <li
              key={`${row.kind}-${row.id}`}
              className={[
                'layer-row',
                `layer-${row.kind}`,
                row.active ? 'active' : '',
                row.visible ? '' : 'hidden',
                row.locked ? 'locked' : '',
                dropTarget === row.id ? 'drop-target' : '',
                pendingNode ? 'assigning' : '',
              ]
                .filter(Boolean)
                .join(' ')}
              style={{ paddingLeft: 6 + row.depth * 16 }}
              data-testid={`layer-row-${row.id}`}
              // Every row is draggable: layers reorder among themselves, nodes
              // reparent into a group or out of one (Task 10.4 RULE 1).
              draggable={!disabled}
              onDragStart={() => {
                setDragLayer(row.kind === 'layer' ? row.id : null);
                setDragRow({ id: row.id, kind: row.kind });
              }}
              onDragEnd={() => {
                setDragRow(null);
                setDragLayer(null);
                setDropTarget(null);
              }}
              onDragOver={(event) => {
                if (!dragRow) return;
                event.preventDefault();
                setDropTarget(row.id);
              }}
              onDragLeave={() => setDropTarget(null)}
              onDrop={(event) => {
                if (row.kind === 'layer' && dragLayer) {
                  onDropLayer(row.id, event);
                } else if (dragRow?.kind === 'node') {
                  onDropNode(
                    { id: row.id, kind: row.kind, canOpen: row.canOpen, layerId: row.layerId },
                    event,
                  );
                } else if (row.kind === 'node' && onAssignNode) {
                  // The row's *own* layer, not "the layer above it": a node
                  // nested inside a group that lives in another layer is listed
                  // with the layer the engine says holds it, and dropping onto
                  // the row must move it to that layer.
                  event.preventDefault();
                  onAssignNode(row.layerId);
                }
              }}
              onClick={() => {
                if (row.kind === 'layer') {
                  const layerRow = layers.find((candidate) => candidate.id === row.id);
                  if (layerRow && !layerRow.active) {
                    onCommand('Active layer', setActiveLayer(row.id));
                  }
                }
              }}
            >
              {(row.kind === 'layer' || row.canOpen) && (
                <button
                  className="disclose"
                  data-testid={
                    row.kind === 'layer' ? `layer-expand-${row.id}` : `group-expand-${row.id}`
                  }
                  disabled={row.kind === 'layer' ? childCount === 0 : !row.canOpen}
                  onClick={(event) => {
                    event.stopPropagation();
                    onToggleExpanded(row.id);
                  }}
                  title={
                    expanded.has(row.id)
                      ? `Collapse ${row.depth === 0 ? 'layer' : 'group'} ${row.name}`
                      : `Expand ${row.depth === 0 ? 'layer' : 'group'} ${row.name}`
                  }
                  aria-label={`${expanded.has(row.id) ? 'Collapse' : 'Expand'} ${row.name}`}
                  aria-expanded={expanded.has(row.id)}
                >
                  {expanded.has(row.id) ? '▾' : row.kind === 'layer' && childCount === 0 ? '·' : '▸'}
                </button>
              )}
              {row.kind === 'node' && <span className="node-dot">{row.isGroup ? '▣' : '◆'}</span>}

              {isRenaming ? (
                <input
                  className="rename"
                  autoFocus
                  value={draftName}
                  onChange={(event) => setDraftName(event.target.value)}
                  onBlur={commitRename}
                  onKeyDown={(event) => {
                    if (event.key === 'Enter') commitRename();
                    if (event.key === 'Escape') setRenaming(null);
                  }}
                />
              ) : (
                <span className="layer-name" onDoubleClick={() => startRename(row)}>
                  {row.name}
                </span>
              )}

              {row.kind === 'layer' && row.active && (
                <span className="chip chip-active" title="New artwork lands here">
                  active
                </span>
              )}
              {row.kind === 'node' && row.ownVisible === false && (
                <span className="chip" title="Hidden by the node's own eye">
                  self-hidden
                </span>
              )}

              <span className="layer-actions">
                {row.kind === 'layer' ? (
                  <>
                    <button
                      className="icon"
                      data-testid={`layer-eye-${row.id}`}
                      disabled={disabled}
                      title={row.visible ? 'Hide layer' : 'Show layer'}
                      aria-label={`${row.visible ? 'Hide' : 'Show'} ${row.kind} ${row.name}`}
                      aria-pressed={!row.visible}
                      onClick={(event) => {
                        event.stopPropagation();
                        onCommand(
                          row.visible ? 'Hide layer' : 'Show layer',
                          setLayerVisible(row.id, !row.visible),
                        );
                      }}
                    >
                      {row.visible ? '👁' : '⃠'}
                    </button>
                    <button
                      className="icon"
                      data-testid={`layer-lock-${row.id}`}
                      disabled={disabled}
                      title={row.locked ? 'Unlock layer' : 'Lock layer'}
                      aria-label={`${row.locked ? 'Unlock' : 'Lock'} ${row.kind} ${row.name}`}
                      aria-pressed={row.locked}
                      onClick={(event) => {
                        event.stopPropagation();
                        onCommand(
                          row.locked ? 'Unlock layer' : 'Lock layer',
                          setLayerLocked(row.id, !row.locked),
                        );
                      }}
                    >
                      {row.locked ? '🔒' : '🔓'}
                    </button>
                    <button
                      className="icon"
                      data-testid={`layer-alpha-${row.id}`}
                      disabled={disabled}
                      title={
                        row.alphaLocked
                          ? 'Unlock alpha — new strokes may leave the layer\'s artwork'
                          : 'Lock alpha — new strokes stay inside what this layer already holds'
                      }
                      aria-label={`${row.alphaLocked ? 'Unlock' : 'Lock'} alpha of layer ${row.name}`}
                      aria-pressed={row.alphaLocked === true}
                      onClick={(event) => {
                        event.stopPropagation();
                        onCommand(
                          row.alphaLocked ? 'Unlock alpha' : 'Lock alpha',
                          setLayerAlphaLocked(row.id, !row.alphaLocked),
                        );
                      }}
                    >
                      {row.alphaLocked ? 'α' : 'a'}
                    </button>
                    <button
                      className="icon"
                      data-testid={`layer-clip-${row.id}`}
                      // The bottom layer has nothing below it to clip to: the
                      // toggle is disabled rather than allowed to mean
                      // "everything disappears".
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
                        onCommand(
                          row.clippingMask ? 'Remove clipping mask' : 'Clip to layer below',
                          setLayerClippingMask(row.id, !row.clippingMask),
                        );
                      }}
                    >
                      {row.clippingMask ? '▤' : '▥'}
                    </button>
                    <button
                      className="icon"
                      data-testid={`layer-rename-${row.id}`}
                      disabled={disabled}
                      title="Rename layer"
                      onClick={(event) => {
                        event.stopPropagation();
                        startRename(row);
                      }}
                    >
                      ✎
                    </button>
                    <button
                      className="icon danger"
                      data-testid={`layer-delete-${row.id}`}
                      disabled={disabled}
                      title="Delete layer (its nodes survive, unassigned)"
                      onClick={(event) => {
                        event.stopPropagation();
                        onCommand('Delete layer', deleteLayer(row.id));
                      }}
                    >
                      ✕
                    </button>
                  </>
                ) : (
                  <span className="layer-flags">
                    {(row.ownVisible ?? true) ? '👁' : '⃠'}
                    {(row.ownLocked ?? false) ? '🔒' : ''}
                  </span>
                )}
              </span>
            </li>
          );
        })}
      </ul>

      {pendingNode && (
        <p className="hint" data-testid="layers-assign-hint">
          Drop the selected node on a layer — or click one — to move it there.
        </p>
      )}
    </section>
  );
}

/** The node-rename builder lives in `commands.ts` but is imported lazily to
 *  keep this component's import list about layers. */
function renameNodeCommand(id: string, name: string): CommandWire {
  return { type: 'RenameNode', id, name };
}

export default LayersPanel;
