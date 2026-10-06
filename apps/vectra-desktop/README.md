# vectra-desktop — the Tauri v2 host (Task 10.0)

> A window around the engine you already have. The React tree, the wasm engine,
> the WebGPU canvas, the AI panel and the exporters are **the same files** the web
> build runs (`apps/vectra-web/src`). What this app adds is what a browser cannot
> do: a `.vectra` file on disk, opened and saved through the **operating
> system's** dialogs.

## Run

```bash
npm install
npm run tauri:dev      # the real thing: native window, native dialogs
npm run tauri:build    # release binary (bundling off: see below)
```

`tauri build` produces `src-tauri/target/release/vectra-desktop` — 10.7 MB
stripped, carrying the whole engine as an embedded (brotli-compressed) wasm
asset. `bundle.active` is `false`: producing deb/rpm/AppImage/msi/dmg artifacts
is a packaging task, not this one. The release profile uses `lto = "thin"`
rather than fat LTO deliberately — see the comment in `src-tauri/Cargo.toml`;
fat LTO + one codegen unit is the usual Tauri recommendation and it was SIGKILLed
by the kernel on this 2-core/1 GB box.

Two other entry points, for frontend work:

```bash
npm run dev            # the shared React tree on http://localhost:5174
npm run typecheck
```

`npm run dev` runs the *same* page (the `@vectra/web` alias points at the web
app's `src`), but outside a Tauri window there is no native host — the file bar
renders disabled and says so. That is deliberate: the desktop host is installed
only when `__TAURI_INTERNALS__` is present, so a browser can never half-use it.

## What is where

| File | What it is |
| --- | --- |
| `src-tauri/src/main.rs` | The Tauri shell: window, plugins, and the commands (`open_document`, `save_document`, `save_document_as`, `verify_document`, `forget_path`, `current_path`, `host_info`). |
| `src-tauri/src/file_io.rs` | RULE 2 (native dialogs) and RULE 3 (`.vectra` = header + gzip) in one place, plus the atomic write. |
| `src-tauri/src/verify.rs` | The Roundtrip Law: parse → plan → replay into a fresh engine → compare summaries. Also the module that explains why the engine is *not* in the host. |
| `src/tauri-host.ts` | The `DocumentHost` implementation the web app's seam expects. |
| `src/main.tsx` | Install the host, mount `App`, and (dev only) run the boot self-test. |
| `src-tauri/tauri.conf.json` | One resizable window, WebGPU-friendly browser args, `bundle.active: false`. |
| `src-tauri/capabilities/default.json` | `core:default` + the four dialog/fs permissions this app actually uses. |

## The `.vectra` file

```
┌──────────┬──────────────┬────────────────────────────────────────────┐
│ "VECTRA" │ u16 LE (=1)  │  RFC 1952 gzip of the Document JSON        │
│ 6 bytes  │ version      │  (header pinned: mtime 0, OS byte 255)     │
└──────────┴──────────────┴────────────────────────────────────────────┘
```

* Identical documents save to **identical bytes** (the gzip header is pinned), so
  a file can be diffed and a test can assert on it.
* A reader refuses a version it does not know, and refuses a payload larger than
  64 MiB *while inflating* — a compression bomb never materialises.
* **A save verifies before it writes** (payload → plan → replay → summary
  comparison, plus a byte-level `decode(encode(json)) == json` check), then writes
  a sibling temp file and renames over the target. A document the engine cannot
  reproduce, or an interrupted write, leaves the previous file intact.
* **An open verifies before it answers**: a file whose replay the engine refuses,
  or whose registry maps and order vectors disagree, never reaches the window.

## Why the engine is not in the host (a finding, not a shortcut)

The obvious design is one engine living in Tauri's managed state. It does not
compile:

```text
error[E0277]: `Rc<RefCell<cassowary::Row>>` cannot be sent between threads safely
```

`cassowary` (Task 3.1's solver) makes `vectra_constraints::Solver` — and with it
`vectra_wasm::VectraEngine` — `!Send`, and `State<T>` needs `T: Send + Sync`.
Task 10.0 RULE 1 forbids refactoring the engine for the host's convenience, so
the host keeps **no** engine: every file operation builds a short-lived engine,
replays the file through it, and drops it. The engine never crosses a thread
because it never leaves its stack frame — and the window's live document stays
the single source of truth, exactly as on the web.

## Verification without a display

The box this was built on has no display, so the launch checks run under Xvfb:

```bash
export XDG_RUNTIME_DIR=/tmp/xdg GDK_BACKEND=x11 \
       WEBKIT_DISABLE_COMPOSITING_MODE=1 WEBKIT_DISABLE_DMABUF_RENDERER=1
xvfb-run -a -s "-screen 0 1440x900x24" ./target/debug/vectra-desktop
```

Debug builds narrate their boot, which is how the run is checked:

```text
vectra-desktop 0.1.0 · linux x86_64 · .vectra v1 · engine: in-window
<< the webview loaded tauri://localhost          (or http://localhost:5174/ in dev)
<< the window asked for host_info — the WebView is up and IPC works
<< verify_document: matches=true commands=1 nodes=1 variables=0 — 1 command(s): 1 node(s), …
```

The last two lines are the point: the React bundle ran, the IPC round trip
worked, and the `.vectra` path (engine → JSON → gzip → replay → compare) ran
inside the real window. What a headless run **cannot** check is the native dialog
itself — that needs a person and a click, and it is the one thing this document
does not claim to have verified.
