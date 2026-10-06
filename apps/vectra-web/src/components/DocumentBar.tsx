/**
 * File ▸ New / Open… / Save / Save As… — the desktop file bar (Task 10.0 RULE 2).
 *
 * This component is deliberately **host-agnostic**: it talks to the
 * `DocumentHost` seam and to the `EngineRunner` seam, and to nothing else. On
 * the web those two resolve to the browser stubs, which *refuse* — so this bar
 * renders in both builds and is simply disabled (with the reason in its tooltip)
 * where there are no native dialogs. That is the Task 10.0 answer to "remove the
 * browser file handling": there is no `<input type="file">` to fall back on, no
 * `FileReader`, no drag-and-drop import. A document is a file on a disk, and
 * only an operating system gets to ask the user where it is.
 *
 * What this component does *not* do: it never looks inside the JSON. Opening
 * hands the plan back to the engine as commands; saving hands the engine's own
 * JSON to the host. Everything between those two sentences is Rust.
 */

import { useCallback, useState } from 'react';
import type { DocumentHost } from '../engine/host';
import { unavailableMessage } from '../engine/host';

export type FileLogKind = 'info' | 'cmd' | 'error' | 'ok';

export interface DocumentBarProps {
  host: DocumentHost;
  /**
   * The live document's JSON, straight from the engine (Task 10.0 RULE 3).
   *
   * A *provider*, not a value: it is called at the moment of the save, so the
   * bytes written are the document as it is then — not as it was when this
   * component last rendered.
   */
  documentJson: () => string;
  /** Replay a loaded file's commands into the page engine (so the canvas draws it). */
  onReplay: (commands: unknown[]) => void;
  onLog: (kind: FileLogKind, text: string) => void;
  /** Disabled until the engine is up: there is nothing to save before that. */
  ready: boolean;
  suggestedName?: string;
}

/** Which operation is in flight (the bar disables itself while one is). */
type Phase = 'idle' | 'new' | 'open' | 'save' | 'save-as' | 'verify';

export function DocumentBar({
  host,
  documentJson,
  onReplay,
  onLog,
  ready,
  suggestedName = 'untitled.vectra',
}: DocumentBarProps) {
  const [phase, setPhase] = useState<Phase>('idle');
  const [path, setPath] = useState<string | null>(host.currentPath);
  const enabled = ready && host.available && phase === 'idle';

  const run = useCallback(
    async (which: Exclude<Phase, 'idle'>, action: () => Promise<void>) => {
      setPhase(which);
      try {
        await action();
      } catch (error) {
        // The host's refusals are sentences meant for a user (a corrupt file, a
        // failed write, an engine refusal) — show them as-is.
        onLog('error', `✗ ${error instanceof Error ? error.message : String(error)}`);
      } finally {
        setPhase('idle');
        setPath(host.currentPath);
      }
    },
    [host, onLog],
  );

  const onNew = useCallback(
    () =>
      run('new', async () => {
        onLog('cmd', '→ File ▸ New');
        await host.newDocument();
        // The page's engine is reset with the window: a fresh WebView starts
        // with a fresh document, and the host has already forgotten the old one.
        onLog('info', 'a new, empty document');
        setPath(null);
        if (host.kind === 'desktop') window.location.reload();
      }),
    [run, host, onLog],
  );

  const onOpen = useCallback(
    () =>
      run('open', async () => {
        onLog('cmd', '→ File ▸ Open…');
        const result = await host.open();
        if (!result) {
          onLog('info', 'open cancelled');
          return;
        }
        onLog('ok', `opened ${result.path} — ${result.headline}`);
        // The file's plan is *commands*, so the page engine rebuilds the
        // document itself (and its own gates validate it a second time).
        onReplay(result.plan);
        setPath(result.path);
      }),
    [run, host, onLog, onReplay],
  );

  const onSave = useCallback(
    (as: boolean) =>
      run(as ? 'save-as' : 'save', async () => {
        onLog('cmd', `→ File ▸ ${as ? 'Save As…' : 'Save'}`);
        // The document comes from the engine — never from this component, which
        // holds no document state by design.
        const json = documentJson();
        if (!json) {
          onLog('error', '✗ the engine has no document to save yet');
          return;
        }
        const result = as
          ? await host.saveAs(json, suggestedName)
          : await host.save(json, suggestedName);
        if (!result) {
          onLog('info', 'save cancelled');
          return;
        }
        const ratio = result.jsonBytes > 0 ? result.bytes / result.jsonBytes : 1;
        onLog(
          'ok',
          `saved ${result.path} — ${result.bytes} bytes (${Math.round(ratio * 100)}% of ` +
            `${result.jsonBytes} JSON bytes), .vectra v${result.format}` +
            (result.overwrote ? ', replacing the previous file' : ''),
        );
        setPath(result.path);
      }),
    [run, host, documentJson, onLog, suggestedName],
  );

  /**
   * **The Roundtrip Law, on demand.** This asks the host to parse the live
   * document, compile its replay plan, rebuild it in a fresh engine and compare
   * the summaries — the same check a save performs before it writes a byte. It
   * is the task's §5 law, made clickable, so it is visible instead of asserted.
   */
  const onVerify = useCallback(
    () =>
      run('verify', async () => {
        onLog('cmd', '→ Verify: the Roundtrip Law on the live document');
        const json = documentJson();
        if (!json) {
          onLog('error', '✗ the engine has no document to verify yet');
          return;
        }
        const check = await host.verify(json);
        if (check.matches) {
          onLog(
            'ok',
            `✓ Roundtrip Law: ${check.headline} — a fresh engine rebuilt this document ` +
              `from its own ${check.commands} command(s), and the summaries match exactly`,
          );
        } else {
          for (const diff of check.diffs) {
            onLog(
              'error',
              `✗ ${diff.field} differs: expected ${diff.expected}, found ${diff.found}`,
            );
          }
        }
        setPath(host.currentPath);
      }),
    [run, host, documentJson, onLog],
  );

  const reason = enabled || !ready ? null : unavailableMessage();
  const file = path ? path.split(/[\\/]/).pop() : suggestedName;

  return (
    <div className="doc-bar" data-testid="doc-bar">
      <div className="doc-menu">
        <span className="doc-label">File</span>
        <button
          type="button"
          data-testid="doc-new"
          className="doc-button"
          disabled={!enabled}
          title={reason ?? 'Start an empty document'}
          onClick={onNew}
        >
          New
        </button>
        <button
          type="button"
          data-testid="doc-open"
          className="doc-button"
          disabled={!enabled}
          title={reason ?? 'Open a .vectra file (native dialog)'}
          onClick={onOpen}
        >
          Open…
        </button>
        <button
          type="button"
          data-testid="doc-save"
          className="doc-button"
          disabled={!enabled}
          title={reason ?? 'Save as .vectra (native dialog the first time)'}
          onClick={() => onSave(false)}
        >
          Save
        </button>
        <button
          type="button"
          data-testid="doc-save-as"
          className="doc-button"
          disabled={!enabled}
          title={reason ?? 'Save under a new name'}
          onClick={() => onSave(true)}
        >
          Save As…
        </button>
        <button
          type="button"
          data-testid="doc-verify"
          className="doc-button doc-button-quiet"
          disabled={!enabled}
          title={
            reason ??
            'Run the Roundtrip Law: rebuild this document from its own commands ' +
              'in a fresh engine and compare the summaries'
          }
          onClick={onVerify}
        >
          Verify
        </button>
      </div>

      <div className="doc-title" data-testid="doc-title" title={path ?? undefined}>
        <span className="doc-name">{file}</span>
        {phase !== 'idle' && <span className="doc-phase">· {phase}…</span>}
        {path === null && <span className="doc-untitled">· unsaved</span>}
      </div>

      <div className="doc-host" data-testid="doc-host">
        <span className={`doc-badge doc-badge-${host.kind}`}>{host.kind}</span>
        <span className="doc-host-note">
          {host.available
            ? 'native dialogs · .vectra (gzip)'
            : 'read-only: the engine demo has no files (Task 10.0)'}
        </span>
      </div>
    </div>
  );
}
