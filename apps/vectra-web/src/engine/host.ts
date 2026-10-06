/**
 * The document host seam (Task 10.0 RULE 2).
 *
 * A *document* is a file on a disk. A browser page has no disk, no dialog, and —
 * by RULE 2 — no `FileReader` either: this module defines what a host must be
 * able to do, the app calls it, and the two implementations that exist are
 *
 * * [`browserHost`] — the honest answer on the web: **not available**. There is
 *   no fallback download, no hidden `<input type="file">`, no `FileReader`.
 *   The web build is a demo of the engine; documents live in the page.
 * * the desktop host, which lives in `apps/vectra-desktop` and is installed
 *   into this seam at startup. It uses Tauri's native dialogs and the Rust
 *   `.vectra` codec, and this module knows nothing about either.
 *
 * The direction of the dependency matters: `vectra-web` never imports a Tauri
 * package, so the browser bundle carries no desktop code, and the desktop app
 * carries the same React tree with a different host plugged in.
 */

/** What the host returns when a file was opened. */
export interface HostOpenResult {
  /** The document as `Document` JSON — what the engine's own save produced. */
  json: string;
  /**
   * The replay plan: the commands that rebuild this document in a fresh
   * engine. The host compiles it *natively* (Rust), so the UI dispatches
   * commands it never had to understand.
   */
  plan: unknown[];
  /** Where it came from, for the title bar and the log. */
  path: string;
  /** `42 command(s): 3 node(s), 1 variable(s), …` — the host's own words. */
  headline: string;
}

/** What the host returns when a file was written. */
export interface HostSaveResult {
  path: string;
  /** Bytes on disk (gzipped) and bytes of JSON, so the win is visible. */
  bytes: number;
  jsonBytes: number;
  /** The `.vectra` container version the file was written in. */
  format: number;
  /** `true` when the file already existed and was replaced. */
  overwrote: boolean;
}

/** The Roundtrip Law's answer for one document (Task 10.0 §5). */
export interface HostVerifyResult {
  /** **The law**: a fresh engine rebuilt this document from its own commands. */
  matches: boolean;
  /** How many commands the replay took. */
  commands: number;
  /** `42 command(s): 3 node(s), …` — the plan's own words. */
  headline: string;
  nodes: number;
  variables: number;
  /** Empty when `matches`; otherwise the sections that disagree. */
  diffs: { field: string; expected: string; found: string }[];
}

/**
 * A place documents can be kept. Every method that could fail resolves with a
 * rejection carrying a message fit to show a user; `null` always means *the
 * user cancelled*, which is not an error.
 */
export interface DocumentHost {
  /** `'browser'` or `'desktop'` — what the UI says in the file bar. */
  readonly kind: 'browser' | 'desktop';
  /** A short human label for the host, e.g. `desktop · macOS`. */
  readonly label: string;
  /** Can this host open and save at all? */
  readonly available: boolean;
  /** The file the document came from / was last saved to. */
  readonly currentPath: string | null;
  /** Start an empty document (File ▸ New) — the host's engine, not the page's. */
  newDocument(): Promise<void>;
  /** Ask the OS for a `.vectra` file and load it. */
  open(): Promise<HostOpenResult | null>;
  /** Ask the OS where to put a new `.vectra` file and write it. */
  saveAs(documentJson: string, suggestedName: string): Promise<HostSaveResult | null>;
  /** Write to the current path (falls back to `saveAs` when there is none). */
  save(documentJson: string, suggestedName: string): Promise<HostSaveResult | null>;
  /**
   * Run the Roundtrip Law on a document JSON string: parse it, compile its
   * replay plan, replay it into a fresh engine, and compare the summaries.
   * This is the *same* function the save path runs before it writes a byte, so
   * asking for it on demand shows exactly what a save would prove.
   */
  verify(documentJson: string): Promise<HostVerifyResult>;
  /** Forget the current path (File ▸ New). */
  reset(): void;
}

/**
 * The web build's host: present, honest, and incapable.
 *
 * Refusing is the point. A browser fallback (`URL.createObjectURL` + a hidden
 * anchor, or a reader on a file input) would look like the feature works while
 * quietly writing a *different* kind of file — one the desktop app would have to
 * guess about. The engine's own export panel covers "get something out of the
 * page"; documents are a disk concept.
 */
export const browserHost: DocumentHost = {
  kind: 'browser',
  label: 'web (no file I/O)',
  available: false,
  currentPath: null,
  newDocument() {
    return Promise.reject(new Error(NOT_AVAILABLE));
  },
  open() {
    return Promise.reject(new Error(NOT_AVAILABLE));
  },
  saveAs() {
    return Promise.reject(new Error(NOT_AVAILABLE));
  },
  save() {
    return Promise.reject(new Error(NOT_AVAILABLE));
  },
  verify() {
    // The web build cannot verify a document because it cannot *be* a document:
    // there is no `.vectra` file to be right or wrong about.
    return Promise.reject(new Error(NOT_AVAILABLE));
  },
  reset() {},
};

const NOT_AVAILABLE =
  'opening and saving .vectra files needs the desktop app — the web build has no ' +
  'file system. Use Export ▸ SVG / React to take code out of the page, or run ' +
  '`npm run tauri dev` in apps/vectra-desktop.';

/** The message the file bar shows when the host cannot keep documents. */
export function unavailableMessage(): string {
  return NOT_AVAILABLE;
}

let installed: DocumentHost = browserHost;

/** Install the host a platform build provides (the desktop app calls this once). */
export function installDocumentHost(host: DocumentHost): void {
  installed = host;
}

/** The host this build has. */
export function documentHost(): DocumentHost {
  return installed;
}
