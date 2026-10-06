/**
 * The desktop entry point.
 *
 * The whole of the wiring, and the point of the task: **the same `App` the
 * browser renders**, with the desktop's document host installed in place of the
 * browser's refusal. Nothing about the engine, the canvas, the drag triad, the
 * AI panel or the exporters changes between the two builds — the desktop build
 * adds a native dialog, a `.vectra` file on disk, and a window around them.
 *
 * The one guard worth explaining: the desktop host is installed **only inside a
 * Tauri window**. This same page also runs under a plain dev server (`npm run
 * dev`, port 5174) for frontend work with hot reload — and there, a host whose
 * `invoke` calls would throw on every click is worse than no host at all. So a
 * browser gets the honest refusal: the file bar renders, disabled, saying that
 * files need the desktop app.
 */
import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import App from '@vectra/web/App';
import { installDocumentHost } from '@vectra/web/engine/host';
import '@vectra/web/App.css';
import { desktopHost, describeHost, hostPath } from './tauri-host';

/** Are we inside the Tauri window (as opposed to a browser on the dev server)? */
const inTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

if (inTauri) {
  installDocumentHost(desktopHost);

  // The host describes itself once: the file bar shows the label it returns
  // rather than guessing a platform, and reports a format version it did not
  // hard-code.
  void describeHost().then((info) => {
    console.info(
      `vectra-desktop · ${info.label} · .vectra v${info.format_version} · engine in-window` +
        (info.resident_engine ? ' + host' : ''),
    );
  });

  // A window reload (File ▸ New) starts with an empty engine but a host that may
  // still know the last document's path — so the two are reconciled once here.
  void hostPath().then((path) => {
    if (path) console.info(`the host's current document is ${path}`);
  });
} else {
  console.info(
    'vectra-desktop: this page is the desktop shell, running outside its Tauri window — ' +
      'file open/save is disabled. Run `npm run tauri:dev` for the real thing.',
  );
}

/**
 * The desktop boot self-test (debug builds only).
 *
 * Task 10.0's file path crosses four things that a headless machine cannot
 * click through: the wasm engine in the WebView, the host's `.vectra` codec, the
 * engine's replay plan, and the IPC between them. So rather than *claim* the path
 * works, this runs it once at startup on a throwaway document — engine → JSON →
 * `verify_document` → gzip → plan → fresh engine → summary comparison — and the
 * host prints the verdict to the terminal, where a headless run can see it.
 *
 * It is off in release builds: a shipped app does not do work nobody asked for.
 */
if (inTauri && import.meta.env.DEV) {
  void (async () => {
    try {
      const { VectraClient } = await import('@vectra/web/engine/client');
      const { verifyDocument } = await import('./tauri-host');
      const engine = await VectraClient.create();
      engine.dispatch({
        type: 'CreateNode',
        id: crypto.randomUUID(),
        name: 'boot-check',
        kind: {
          Rectangle: {
            x: { Literal: 0 },
            y: { Literal: 0 },
            width: { Literal: 80 },
            height: { Literal: 60 },
            corner_radius: { Literal: 0 },
          },
        },
      });
      const check = await verifyDocument(engine.documentJson());
      console.info(
        `vectra-desktop boot self-test: roundtrip=${check.matches} commands=${check.commands} ` +
          `nodes=${check.nodes} · ${check.headline}`,
      );
    } catch (error) {
      console.error('vectra-desktop boot self-test failed', error);
    }
  })();
}

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
