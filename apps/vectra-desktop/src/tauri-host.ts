/**
 * The desktop document host (Task 10.0 RULE 2 + RULE 3), installed into the
 * app's `DocumentHost` seam at startup.
 *
 * Everything native happens through `invoke`, which posts a message to the Rust
 * side of the window. There is no `fetch`, no `Blob`, no `FileReader` and no
 * `<input type="file">` in this file — RULE 2 removed them. A file dialog here
 * is the *operating system's*, and the bytes never pass through JavaScript: the
 * host reads the file, gunzips it, and hands over a `Document` JSON string; it
 * takes a string back on save and writes the gzipped container itself.
 *
 * ## What this file deliberately does *not* do: hold a document
 *
 * The engine is in this window's wasm module, exactly as on the web, and the
 * host has no second copy — see `src-tauri/src/verify.rs` for why that is a
 * finding (`cassowary` makes the engine `!Send`, so it cannot live in Tauri's
 * managed state) and not a simplification. The consequences shape this file:
 *
 * * **open** returns the document's *commands* (`plan`), and the app replays them
 *   into the window's engine. A loaded file is therefore validated by the engine
 *   on the way in, twice: once by the host (which refuses to hand over a file
 *   whose replay does not reproduce it) and once by the window as it applies the
 *   same commands.
 * * **save** takes the JSON from the window's engine. The host encodes, verifies
 *   and writes it, and reports the round trip it performed.
 * * **new** forgets the path and reloads the window: an empty document is the
 *   absence of a document, and a fresh WebView is the honest way to get one.
 */

import { invoke } from '@tauri-apps/api/core';
import type {
  DocumentHost,
  HostOpenResult,
  HostSaveResult,
  HostVerifyResult,
} from '@vectra/web/engine/host';

/** What `host_info` answers — the host's own description of itself. */
export interface HostInfoWire {
  kind: 'desktop';
  os: string;
  arch: string;
  format_version: number;
  format_extension: string;
  webgpu_expected: boolean;
  label: string;
  /**
   * Always `false`: the host keeps no engine, because the engine is `!Send`.
   * Reported as data so the UI can say it out loud instead of implying a second
   * document exists somewhere.
   */
  resident_engine: boolean;
}

/** What the Rust `save_document` answers. */
interface SaveReportWire {
  path: string;
  bytes: number;
  json_bytes: number;
  ratio: number;
  format_version: number;
  overwrote: boolean;
  roundtrip: boolean;
  headline: string;
}

/** What the Rust `open_document` answers. */
interface OpenReportWire {
  path: string;
  json: string;
  plan: unknown[];
  headline: string;
  nodes: number;
  variables: number;
  format_version: number;
  bytes: number;
  summary: string;
}

/** The Roundtrip Law's answer for a JSON string (`verify_document`). */
export interface DocumentCheckWire {
  matches: boolean;
  commands: number;
  headline: string;
  plan: unknown[];
  nodes: number;
  variables: number;
  expressions: number;
  constraints: number;
  operations: number;
  tracks: number;
  procedural: number;
  summary: string;
  diffs: { field: string; expected: string; found: string }[];
}

let info: HostInfoWire | null = null;

/** The host's self-description, fetched once (constant for the process). */
export async function describeHost(): Promise<HostInfoWire> {
  if (!info) info = await invoke<HostInfoWire>('host_info');
  return info;
}

/** The current path is the host's; the UI keeps a copy for the file bar. */
let currentPath: string | null = null;

/** Where the document came from, for the title bar. */
export function desktopPath(): string | null {
  return currentPath;
}

/** The Roundtrip Law, on demand, for any document JSON string. */
export async function verifyDocument(documentJson: string): Promise<DocumentCheckWire> {
  return invoke<DocumentCheckWire>('verify_document', { documentJson });
}

/** The desktop document host. */
export const desktopHost: DocumentHost = {
  kind: 'desktop',
  label: 'desktop',
  available: true,
  get currentPath() {
    return currentPath;
  },

  async newDocument(): Promise<void> {
    // Forget where the last document lived; the caller reloads the window, so
    // the engine starts empty. (The next Save therefore asks for a name instead
    // of silently overwriting the file the user just left.)
    await invoke('forget_path');
    currentPath = null;
  },

  async open(): Promise<HostOpenResult | null> {
    const report = await invoke<OpenReportWire | null>('open_document');
    if (!report) return null;
    currentPath = report.path;
    return {
      json: report.json,
      plan: report.plan,
      path: report.path,
      headline: report.headline,
    };
  },

  async save(documentJson: string, suggestedName: string): Promise<HostSaveResult | null> {
    const report = await invoke<SaveReportWire | null>('save_document', {
      documentJson,
      suggestedName,
    });
    return report ? finishSave(report) : null;
  },

  async saveAs(documentJson: string, suggestedName: string): Promise<HostSaveResult | null> {
    const report = await invoke<SaveReportWire | null>('save_document_as', {
      documentJson,
      suggestedName,
    });
    return report ? finishSave(report) : null;
  },

  /** The Roundtrip Law on the document the window is holding. */
  async verify(documentJson: string): Promise<HostVerifyResult> {
    const check = await verifyDocument(documentJson);
    return {
      matches: check.matches,
      commands: check.commands,
      headline: check.headline,
      nodes: check.nodes,
      variables: check.variables,
      diffs: check.diffs,
    };
  },

  reset() {
    currentPath = null;
  },
};

function finishSave(report: SaveReportWire): HostSaveResult {
  currentPath = report.path;
  return {
    path: report.path,
    bytes: report.bytes,
    jsonBytes: report.json_bytes,
    format: report.format_version,
    overwrote: report.overwrote,
  };
}

/** The path the host believes it owns, for reconciling after a reload. */
export async function hostPath(): Promise<string | null> {
  const path = await invoke<string | null>('current_path');
  currentPath = path;
  return path;
}
