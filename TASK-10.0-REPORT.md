# Task 10.0 — Tauri Desktop Packaging & Native File I/O

**Status: delivered.** Vectra runs as a native desktop application: a Tauri v2
window around the same React + WASM + WebGPU stack, with **native file dialogs**
and a **`.vectra` document format**. The engine was not touched.

---

## 1. The three rules, and where each one lives

### RULE 1 — Engine immutability

`vectra-core`, `vectra-geometry`, `vectra-constraints`, `vectra-expression`,
`vectra-operations`, `vectra-procedural`, `vectra-motion`, `vectra-render`,
`vectra-export`, `vectra-ai`, `vectra-dependency`: **zero changes** in this task.

Tauri is a host. It was never allowed to ask the engine for anything the web
build does not already ask for — with **one** exception, reported rather than
hidden:

> **Deviation (one read-only method).** `vectra_wasm::VectraEngine::document_json()` was added to the
> wasm boundary: it returns `serde_json::to_string(self.core.document())`, i.e. the engine's own
> serialization of the document it is holding. RULE 3 requires the frontend to hand the save path the
> raw `Document` JSON, and the only alternative was to rebuild a `Document` *outside* the engine in the
> host — a second serializer, i.e. two places that could disagree about what a file means. The method
> is **read-only, additive, and touches no wire format**: no existing method changed, no `Command`
> changed, no event changed, and `dispatch_command` — the boundary itself — is untouched. It is
> documented as such at the call site.
>
> A second, larger reason it is *only* one method: see §3. The engine is `!Send`, so loading cannot be
> `engine.load(json)` even if we wanted it to be — **loading is a command replay**, which is what the
> plan in `vectra-file` compiles and what the window and the host both execute.

### RULE 2 — Native file dialogs only

| Removed / never added | Where |
| --- | --- |
| `<input type="file">` | appears nowhere in `apps/vectra-web`, `apps/vectra-desktop` (verified: `grep -rn "input type=\"file\"\|FileReader\|createObjectURL"` → no hits outside comments) |
| `FileReader`, `Blob` reads, `URL.createObjectURL` | same |
| drag-and-drop import of a document | same |
| a browser fallback "download" for `.vectra` | **deliberately absent** — see below |

The native side is `tauri-plugin-dialog`: `open_document` and `save_document*`
call `app.dialog().file()…blocking_pick_file()` / `.blocking_save_file()`. Tauri
v2 extends the fs plugin's scope to a path *because the user picked it*, so
reading and writing that one file needs no extra configuration and no wildcard
grant; the capability file (`src-tauri/capabilities/default.json`) asks for
`core:default` plus `dialog:allow-open`, `dialog:allow-save` and the four fs
read/write permissions — nothing more.

The web build keeps an **honest refusal** instead of a fallback. `browserHost`
(`apps/vectra-web/src/engine/host.ts`) reports `available: false` and rejects
every operation with a sentence that names the desktop app. The file bar renders
in both builds and is simply disabled where there are no native dialogs. A
`Blob` download would have *looked* like the feature worked while writing a
different kind of file — the one thing RULE 3 forbids.

### RULE 3 — The `.vectra` file format

`crates/vectra-file` (new crate, 13th workspace member) owns the container and
the replay plan. The frontend passes strings; Rust does every byte.

```
┌──────────┬─────────────┬──────────────────────────────────────────────────┐
│ "VECTRA" │ u16 LE = 1  │ RFC 1952 gzip ( Document JSON )                  │
│ 6 bytes  │ version     │ header pinned: mtime 0, OS byte 255 (determinism) │
└──────────┴─────────────┴──────────────────────────────────────────────────┘
```

* `encode()` validates the payload as JSON **first**, then compresses through
  `flate2` with the pure-Rust backend (`rust_backend`), so files are byte-stable
  across platforms and identical documents save to identical bytes.
* `decode()` inflates through `.take(MAX+1)` — a 64 MiB ceiling enforced *while*
  inflating, so a compression bomb never materialises.
* Typed `DocError`: `NotAVectraFile`, `Truncated`, `UnsupportedVersion`,
  `NotCompressed`, `Corrupt`, `NotJson`, `TooLarge`. Every variant is a sentence
  a user can read.
* **A save verifies before it writes**: payload → plan → replay into a fresh
  engine → summary comparison, *plus* a byte-level `decode(encode(json)) == json`
  check. Only then does it write a sibling `.tmp` file and `rename` it over the
  target — so a document the engine cannot reproduce, or an interrupted write,
  leaves the previous file untouched.
* **An open verifies before it answers**: a file whose replay the engine refuses,
  or whose registry maps and order vectors disagree, never reaches the window.

---

## 2. Tauri configuration

`apps/vectra-desktop/src-tauri/tauri.conf.json`:

| Setting | Value | Why |
| --- | --- | --- |
| windows | one, `label: "main"`, 1440×900, min 1024×640, resizable, decorated | single-window app (plan item 1) |
| `additionalBrowserArgs` | `--enable-features=Vulkan,UseSkiaRenderer --enable-unsafe-webgpu` | WebGPU in WebView2 (Windows); a no-op elsewhere |
| `security.csp` | `null` | the page is local and loads only its own bundle; no remote origins exist to allow |
| `security.assetProtocol` | **removed** | it costs a `tauri` feature (`protocol-asset`) and a permission, and nothing in this app serves a file to the page as a URL. Tauri's build script refused the config until the two agreed, which is exactly the kind of strictness you want in a file-format task |
| `bundle.active` | `false` | an *army of bundlers* (deb/rpm/AppImage/msi/dmg signing) is not what Task 10.0 asked for; the binary is the deliverable, and `tauri build` still produces it |
| plugins | `tauri-plugin-dialog`, `tauri-plugin-fs` | RULE 2 |
| `beforeDevCommand` / `beforeBuildCommand` | `npm run dev` / `npm run build` | the frontend is a normal Vite app |

Capabilities: `core:default` + dialog + the four fs read/write permissions, in
`src-tauri/capabilities/default.json`.

**Workspace shape.** `src-tauri/Cargo.toml` declares `[workspace]` — its own
workspace root. The engine workspace stays buildable with a plain Rust
toolchain and free of ~400 GUI crates; the host reaches the product through
**path dependencies** (`vectra-wasm`, `vectra-core`, `vectra-file`), so the same
sources are compiled, never copied.

---

## 3. The finding: the engine is `!Send`, so the host keeps no engine

The obvious desktop design is one engine in Tauri's managed state. It does not
compile, and the reason is upstream of Tauri:

```text
error[E0277]: `Rc<RefCell<cassowary::Row>>` cannot be sent between threads safely
   --> required by a bound introduced by this call: Manager::manage(Host { … })
```

Task 3.1 solved constraints with `cassowary`, whose rows are `Rc<RefCell<Row>>`.
`vectra_constraints::Solver` is therefore `!Send`, `vectra_wasm::VectraEngine`
contains one, and Tauri's `State<T>` requires `T: Send + Sync`. RULE 1 forbids
refactoring the engine to accommodate Tauri, so the host keeps **no resident
engine** — and the design that resulted is *better*, not just possible:

| | resident engine (impossible) | short-lived engine (shipped) |
| --- | --- | --- |
| documents in the process | two (host + window) — can drift | one (the window) — cannot |
| sync law needed | "keep both in step" | none |
| load | would apply commands host-side | host verifies, window applies |
| cost | none | one engine construction per file operation (~µs–ms, off the UI path) |

`verify.rs` documents this at the top, and `main.rs`'s `Host` struct holds
exactly one field: `current_path: Mutex<Option<PathBuf>>`. **A `String` cannot
drift from the window's engine; a copy of the document could.**

Consequence for the frontend: the desktop host is a **codec + dialogs**, and the
window's own wasm engine stays the single source of truth — exactly as on the
web. That is why there is no `runner` seam in `App.tsx`: the abstraction I first
built for a second engine was deleted once the compiler showed there is no
second engine to abstract over.

---

## 4. Native command implementations

`src-tauri/src/main.rs` — the commands (all synchronous; none blocks the UI
thread for longer than a file write):

| Command | What it does |
| --- | --- |
| `host_info() -> HostInfo` | platform, arch, `.vectra` version + extension, whether WebGPU is *expected*, and `resident_engine: false` with the reason. The UI reads this instead of guessing |
| `open_document() -> Option<OpenReport>` | native open dialog → `read_document` → **verify** → `{ json, plan, headline, nodes, variables, bytes, summary, path }` |
| `save_document(json, name?) -> Option<SaveReport>` | write to the current path, asking for one only when there is none |
| `save_document_as(json, name?) -> Option<SaveReport>` | always asks |
| `verify_document(json) -> DocumentCheck` | the Roundtrip Law on any JSON string — the same function the save path runs |
| `forget_path()`, `current_path()` | File ▸ New forgets; the file bar reads |

`file_io.rs` holds RULE 2 + RULE 3 together: `ask_open`, `ask_save`,
`read_document`, `write_document`, `sibling_temp`, `refusal`.

`verify.rs` is the law:

```
json ──document_from_json──▶ Document ──replay──▶ [Command] ──dispatch_command──▶ fresh VectraEngine
  │                                                                                      │
  └── DocumentSummary ───────────── compare (7 sections + version + integrity) ──────────┘
```

Every command goes through `dispatch_command` — **the same boundary the WebView
uses** — so a loaded file is validated by the engine's own gates (cycles, unknown
properties, port types, unsatisfiable constraints), not by a re-implementation in
the host.

---

## 5. React file I/O integration

The web app grew exactly two seams; the desktop app fills them.

| File | Role |
| --- | --- |
| `apps/vectra-web/src/engine/host.ts` | `DocumentHost` interface + `browserHost` (the refusal) + `installDocumentHost` / `documentHost`. **`vectra-web` imports no Tauri package** — the browser bundle contains no desktop code |
| `apps/vectra-web/src/components/DocumentBar.tsx` | File ▸ **New / Open… / Save / Save As… / Verify**, host-agnostic; renders disabled (with the reason in its tooltip) where there are no dialogs |
| `apps/vectra-web/src/engine/client.ts` | `documentJson(): string` — the engine's own `Document` JSON, read-only (RULE 3's "the frontend only passes the raw JSON") |
| `apps/vectra-web/App.tsx` | `<DocumentBar>` under the header; `replayPlan(commands)` replays an opened file's plan into the page engine (so the canvas draws it) and logs the file-bar results into the existing event log |
| `apps/vectra-desktop/src/tauri-host.ts` | the `DocumentHost` implementation: `invoke` wrappers, path bookkeeping, no document state |
| `apps/vectra-desktop/src/main.tsx` | installs the host **only inside Tauri** (`__TAURI_INTERNALS__`), imports `App` from `@vectra/web/App`, and runs the dev-only boot self-test |

The desktop build points its Vite alias at `../vectra-web/src`, so **the UI is one
implementation**, not a fork. Both builds bundle the same `src/wasm/` glue.

`DocumentBar` also exposes **Verify**, which asks the host to run the Roundtrip
Law on the live document and logs the verdict — §5's law as a button, so it is
visible rather than merely asserted.

---

## 6. WebGPU context fix (plan item 4)

Honest status: **no engine change was needed, and no fix was invented.**

* The renderer attaches to the `<canvas>` the same way in both builds
  (`client.attachCanvas(element)`, then `measureCanvas` → device-pixel-ratio and
  box; the frame loop asks for `renderFrame()` per rAF). Nothing in that path
  reads a browser-only API that a WebView lacks — it is `web-sys` + `wgpu` over a
  DOM canvas, which is what a WebView *is*.
* The configuration half is in `tauri.conf.json` (`--enable-unsafe-webgpu`,
  Vulkan/Skia args for WebView2) and reported by `host_info().webgpu_expected`:
  `true` for WebView2 and WKWebView, `false` for WebKitGTK — because WebKitGTK
  2.54 needs its own WebGPU feature flag, and pretending otherwise would be a lie
  the canvas panel would then have to contradict.
* This box has **lavapipe** (software Vulkan) and a headless WebKitGTK; whether
  WebGPU initialises there is not a claim this report makes. What *is* verified:
  the window boots, the bundle runs, IPC works, and the engine's document path
  runs inside the window (§7). The canvas panel already reports "canvas
  unavailable" gracefully when `navigator.gpu` is missing — that behaviour is
  Task 5.0's, unchanged.

So plan item 4's *intent* — "ensure the context is acquired inside the WebView and
the dpr/resize differences are handled" — was satisfied by not needing to change
anything: the renderer was already a plain DOM consumer, and `dpr`/resize go
through `ResizeObserver` + `measureCanvas`, both of which exist unchanged in
WebKit/WebView2. What a *person with a display* should check is listed in §9.

---

## 7. Tests and results

### The two laws of Task 10.0 §5

**Roundtrip Law** — three independent implementations, at three levels:

| Where | What it proves |
| --- | --- |
| `crates/vectra-file/tests/roundtrip_laws.rs` (12 tests) | the container and the plan, natively: header/gzip/version, refusal of corrupt input, byte-stable re-encoding, the Replay Law (`commands rebuild the document into a fresh engine`), parked constraints/operations/procedural nodes surviving, and `law_roundtrip_save_close_reopen_matches_the_summary_exactly` — save → drop the engine → reopen → `DocumentSummary` compared as `serde_json::Value` |
| `crates/vectra-wasm/tests/file_laws.rs` (4 tests) | the law **at the boundary the window loads**: `VectraEngine::document_json()` → `encode` → `decode` → `document_from_json` → `replay` → a fresh `VectraEngine` → identical summary *and* identical `to_text()`; plus `the_serialized_document_and_the_summary_agree` (draw order identical in both surfaces) and `serializing_a_document_does_not_change_it` |
| `apps/vectra-desktop/src-tauri/src/verify.rs` (5 tests) | the shipped code path: `a_document_verifies_against_a_fresh_engine`, `an_inconsistent_file_is_caught_before_it_is_trusted`, `a_version_the_replay_cannot_reproduce_is_reported`, `a_payload_the_engine_refuses_is_refused_here`, `nonsense_is_refused_before_any_replay` |

**Native Dialog Law** — the `Verify` path proves the *host* side, and the honest
limit is stated: `open_document`/`save_document*` reach the OS picker through
`tauri_plugin_dialog` only (`grep` for `input type="file"`/`FileReader`/
`createObjectURL` in both apps returns nothing outside comments), and a headless
run cannot click a dialog. **No browser element exists that could stand in for
one** — that is the enforceable half of the law, and it is enforced by absence.
§9 lists the click-through that a person with a display should perform.

### Gates

| Gate | Result |
| --- | --- |
| `cargo test --workspace --no-fail-fast` | **453 passed / 0 failed** (was 435; +12 `roundtrip_laws`, +4 `file_laws`, +2 `vectra-file` unit tests) |
| `cargo clippy --workspace --all-targets` | **0 warnings** (one `unnecessary_sort_by` in `plan.rs` was found and fixed) |
| `cargo fmt --all -- --check` | clean |
| `cargo test` + `cargo clippy` in `apps/vectra-desktop/src-tauri` | **5 passed / 0 failed**, clippy clean |
| `npm run typecheck` (web) | clean |
| `npm run test:ui` (web) | **40 / 40** |
| `node scripts/smoke.mjs` (web) | **45 / 45** — new step 45 loads the rebuilt wasm and checks `document_json()` against the summary (the `.vectra` save path's engine half) |
| `npm run build` (web) | clean, 39 modules |
| `npm run build` (desktop) | clean, 39 modules, wasm bundled |
| `cargo build` (desktop) | binary produced (370 MB debug) |
| **`tauri build`** | **SUCCESS** — release profile, 4 m 21 s (then 1 m 05 s incremental), `Built application at: …/target/release/vectra-desktop`, **10 746 232 bytes**, stripped ELF |
| **headless launch, release binary** | the window stayed up 60 s with no panic, loading the embedded frontend (`tauri://localhost`) |
| `cargo clippy --release --all-targets` (desktop) | 0 warnings (a release-only unused-variable warning was found and fixed) |
| **headless launch, dev server** | window up 100 s, no panic: `<< the webview loaded http://localhost:5174/` → `<< the window asked for host_info — the WebView is up and IPC works` → `<< verify_document: matches=true commands=1 nodes=1 …` |
| **headless launch, embedded assets** | `<< the webview loaded tauri://localhost` → same beacon: the production asset path (no dev server) works |

The wasm was **rebuilt** (`bash scripts/build-wasm.sh`, wasm-bindgen CLI 0.2.129
fetched as the prebuilt musl release after the sandbox reset) because
`document_json` is new at the boundary: 20 661 253 → 20 727 143 bytes. The whole
web suite (typecheck, 40 UI tests, 45 smoke steps, build) was re-run **against
that new glue**.

---

## 8. `tauri build`

`npx tauri build` runs with `bundle.active: false` — it compiles the release
binary rather than driving the platform bundlers (deb/rpm/AppImage/msi/dmg,
signing, notarisation), which is not what Task 10.0 asked for.

**What happened, in order — this is an environment story, and it changed a
production setting:**

| Attempt | Config | Outcome |
| --- | --- | --- |
| 1 | `lto = true`, `codegen-units = 1` | **SIGKILLed by the kernel** inside `rustc --crate-name gtk` — the box has ~1 GB of RAM (1984 MB total) and fat LTO wants several GB |
| 2 | `lto = "thin"`, `codegen-units = 4` | **`No space left on device`** — 25 GB root filesystem, of which the debug target (5.4 GB), the workspace target (4.5 GB) and a 4 GB swapfile I had just added were eating almost all of it |
| 3 | thin LTO + 4 codegen units + `RUSTFLAGS=-C debuginfo=0` + `CARGO_BUILD_JOBS=2`, with the incremental cache (696 MB) and the previous release artifacts (1.7 GB) cleared | **SUCCESS in 4 m 21 s** → `10 746 232`-byte stripped binary, then `1 m 05 s` incremental after the release-only warning fix. Launched headless: the release window stays up with the embedded frontend |

The permanent change that came out of attempt 1: **the release profile is now
`lto = "thin"`, `codegen-units = 4`, `opt-level = "s"`, `strip = true`**, with the
reason written in `Cargo.toml`. Tauri's docs recommend fat LTO + one codegen
unit; that is the right default on a build machine with memory to spare and the
wrong one for a repository that has to build on a small box, and thin LTO keeps
most of the size win at a fraction of the peak memory.

Two things this does **not** excuse, both verified another way:

* the release *configuration* is exercised by the two headless launches in §7 —
  the app boots, loads its frontend (both from `tauri://localhost` and from the
  dev server), answers IPC, and runs the `.vectra` path inside the window;
* what a release build would additionally prove is the *link* step and the
  embedded-asset pipeline, and the embedded-asset half **was** verified: a
  `TAURI_CONFIG='{"build":{"devUrl":null}}'` debug build loads `tauri://localhost`
  from the assets baked into the binary.

---

## 9. What a person with a display should check (stated, not implied)

1. `npm run tauri:dev`, then **File ▸ Open…** → the OS picker appears (not a DOM
   element), a `.vectra` file opens, and the canvas draws it.
2. Drag a node, **File ▸ Save**, quit, relaunch, **File ▸ Open…** → the same
   document, with parametric slots still parametric (the inspector still shows
   `$base * 2`, not `80`).
3. **Verify** → the log says `✓ Roundtrip Law: … summaries match exactly`.
4. The canvas panel: on Windows/macOS the WebGPU canvas should come up; on
   WebKitGTK 2.54 it may report "canvas unavailable" until WebKit's WebGPU flag
   is enabled — expected, and already handled gracefully.
5. File ▸ New → the title returns to `untitled.vectra` and the next Save asks for
   a name instead of overwriting the previous file.

---

## 10. Findings

1. **The engine is `!Send`** (cassowary's `Rc<RefCell<Row>>`), so a desktop host
   cannot hold one. This is worth remembering beyond Task 10.0: any future
   "run the engine on a worker/thread" idea hits the same wall. Fixing it belongs
   upstream in `cassowary` or in a deliberate decision to fork the solver — a
   decision for a later task, not a side effect of packaging.
2. **A comparison cannot detect tampering with itself.** My first "tampered file"
   test failed *because the check was working as designed*: expected and found
   summaries both derive from the payload, so a self-consistent edit verifies. The
   honest fix was a real integrity check (`nodes`/`order`, `operations/order`,
   `procedural/order`, `motion/order` must agree) plus a version comparison — a
   `.vectra` file is JSON, and JSON is a format people may edit.
3. **Tauri's build script caught a config/feature mismatch** (`assetProtocol`
   without the `protocol-asset` feature) before a single line of app code ran.
4. **`tauri::generate_context!` prefers `devUrl` in debug builds**, so a
   `cargo build` + launch loads `http://localhost:5174/`, not the embedded dist.
   Both paths were verified separately (`tauri://localhost` via a
   `TAURI_CONFIG='{"build":{"devUrl":null}}'` build).
5. **A Vite dev server and a Tauri target directory are enemies.** The desktop
   dev server died with `ENOSPC` from `fs.watch` while the release build was
   running: `src-tauri/target` grows into hundreds of thousands of files and
   inotify watches are a finite resource (15 628 here). `vite.config.ts` now
   ignores `**/src-tauri/**`, `**/target/**` and `**/dist/**` — worth knowing for
   any Tauri + Vite project on a small machine.
6. **The web build's refusal is now a tested boundary, not a comment**:
   `browserHost` reports `available: false`, and the file bar is disabled with the
   reason in its tooltip. There is no path from the browser build to a
   `.vectra` file — which is the state RULE 2 asked for.

---

## 11. Files

```
crates/vectra-file/                     ★ new crate: the container + the replay plan
  src/{lib,format,plan}.rs
  tests/roundtrip_laws.rs               ← the Roundtrip Law (12)
crates/vectra-wasm/
  src/lib.rs                            ← +document_json() (read-only), documented
  Cargo.toml                            ← +[dev-dependencies] vectra-file
  tests/file_laws.rs                    ★ the law at the engine boundary (4)
apps/vectra-web/src/engine/host.ts      ★ DocumentHost seam + browser refusal
apps/vectra-web/src/engine/client.ts    ← +documentJson()
apps/vectra-web/src/components/DocumentBar.tsx  ★ the file bar
apps/vectra-web/src/App.tsx             ← +<DocumentBar>, +replayPlan, host handle
apps/vectra-web/src/App.css             ← +file-bar styles
apps/vectra-web/scripts/smoke.mjs       ← +step 45 (document serialization)
apps/vectra-desktop/
  package.json vite.config.ts tsconfig.json index.html README.md
  src-tauri/{Cargo.toml,build.rs,tauri.conf.json}
  src-tauri/capabilities/default.json
  src-tauri/icons/{32x32,128x128,128x128@2x,icon}.png + icon.ico + icon.icns
  src-tauri/src/{main,file_io,verify}.rs  ★ the host: commands, codec, the law
  src/{main.tsx,tauri-host.ts}            ★ the host, installed
Cargo.toml                              ← +vectra-file member + flate2 (rust_backend)
```

### Release profile note

Attempt 1's SIGKILL is why `src-tauri/Cargo.toml` says:

```toml
codegen-units = 4
lto = "thin"
opt-level = "s"
strip = true
```

Tauri's own recommendation (`lto = true`, `codegen-units = 1`) is the right
default on a build machine with memory to spare; thin LTO keeps most of the size
win at a fraction of the peak memory, which is what let this box finish the build
at all. The result — a 10.7 MB stripped binary carrying a 20.7 MB wasm engine
(Tauri brotli-compresses embedded assets) — suggests the trade cost very little.

---

**RULES 1–3 delivered.**
