//! The desktop host's Rust side (Task 10.0).
//!
//! A Tauri v2 shell: one window, native file dialogs, and the `.vectra`
//! container codec. The **engine is not here** — see [`verify`] for why that is
//! a finding rather than a shortcut — so the window keeps the live document in
//! its own (wasm) engine, exactly like the web build, and the host's job is the
//! part a browser cannot do:
//!
//! * `open_document` — OS picker → read → gunzip → **verify by replay** → JSON +
//!   the command plan that rebuilds the document into the window's engine;
//! * `save_document` / `save_document_as` — OS picker → encode → **verify** →
//!   atomic write → the round-trip result;
//! * `verify_document` — the Roundtrip Law on any JSON string, on demand.
//!
//! Every one of those goes through `vectra-file` and a short-lived
//! `VectraEngine`, so the file format has exactly one implementation (Rust) and
//! the engine exactly one compiler (the same sources the WebView's wasm build
//! uses).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod file_io;
mod verify;

use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;
use tauri::{Manager, State};

use file_io::{DocumentCheck, DocumentFile, OpenReport, SaveReport};

/// The only state the host keeps: where the current document lives.
///
/// Deliberately not a document. A `String` cannot drift from the window's
/// engine; a copy of the document could.
pub struct Host {
    current_path: Mutex<Option<PathBuf>>,
}

impl Host {
    fn path(&self) -> Result<Option<PathBuf>, String> {
        Ok(self
            .current_path
            .lock()
            .map_err(|_| "host lock poisoned")?
            .clone())
    }

    fn set_path(&self, path: Option<PathBuf>) -> Result<(), String> {
        *self.current_path.lock().map_err(|_| "host lock poisoned")? = path;
        Ok(())
    }
}

/// What the UI asks about the host once, at startup: it should not have to guess
/// its platform, and it should not hard-code a format version.
#[derive(Debug, Clone, Serialize)]
pub struct HostInfo {
    /// `"desktop"` — the web build answers `"browser"` from its own host.
    kind: &'static str,
    /// `linux`, `macos`, `windows`.
    os: &'static str,
    arch: &'static str,
    /// The `.vectra` container version this build reads and writes.
    format_version: u16,
    format_extension: &'static str,
    /// `true` when the WebView is *expected* to expose WebGPU (the canvas panel
    /// reports the truth either way).
    webgpu_expected: bool,
    /// The webview this build ships with, as a sentence.
    label: String,
    /// Whether this host keeps a resident engine. `false`, and why: the engine
    /// is `!Send` (cassowary), so it stays in the window and the host verifies
    /// per call. Reported so the UI can say it out loud instead of implying a
    /// second document exists.
    resident_engine: bool,
}

#[tauri::command]
fn host_info() -> HostInfo {
    // A boot beacon, debug builds only: `tauri dev` prints this line once the
    // window has loaded the React bundle and asked the host who it is talking to.
    // It is the cheapest end-to-end proof that the IPC round trip works, and it
    // is how this build was verified on a machine with no display.
    #[cfg(debug_assertions)]
    println!("<< the window asked for host_info — the WebView is up and IPC works");
    let (label, webgpu_expected) = match std::env::consts::OS {
        "windows" => ("desktop · Windows (WebView2)".to_string(), true),
        "macos" => ("desktop · macOS (WKWebView)".to_string(), true),
        other => (format!("desktop · {other} (WebKitGTK)"), false),
    };
    HostInfo {
        kind: "desktop",
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        format_version: vectra_file::FORMAT_VERSION,
        format_extension: vectra_file::EXTENSION,
        webgpu_expected,
        label,
        resident_engine: false,
    }
}

/// File ▸ Open…: OS dialog → read → gunzip → verify → plan.
#[tauri::command]
fn open_document(
    app: tauri::AppHandle,
    state: State<'_, Host>,
) -> Result<Option<OpenReport>, String> {
    let picked = match file_io::ask_open(&app) {
        Ok(Some(path)) => path,
        Ok(None) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let (file, report) = file_io::read_document(&picked)?;
    state.set_path(Some(file.path))?;
    Ok(Some(report))
}

/// File ▸ Save: write to the current path, or ask for one when there is none.
#[tauri::command]
fn save_document(
    app: tauri::AppHandle,
    state: State<'_, Host>,
    document_json: String,
    suggested_name: Option<String>,
) -> Result<Option<SaveReport>, String> {
    let target = match state.path()? {
        Some(path) => path,
        None => match pick_save_target(&app, suggested_name)? {
            Some(path) => path,
            None => return Ok(None),
        },
    };
    let report = file_io::write_document(&DocumentFile {
        json: document_json,
        path: target.clone(),
    })?;
    state.set_path(Some(target))?;
    Ok(Some(report))
}

/// File ▸ Save As…: always ask, even when a path is known.
#[tauri::command]
fn save_document_as(
    app: tauri::AppHandle,
    state: State<'_, Host>,
    document_json: String,
    suggested_name: Option<String>,
) -> Result<Option<SaveReport>, String> {
    let target = match pick_save_target(&app, suggested_name)? {
        Some(path) => path,
        None => return Ok(None),
    };
    let report = file_io::write_document(&DocumentFile {
        json: document_json,
        path: target.clone(),
    })?;
    state.set_path(Some(target))?;
    Ok(Some(report))
}

/// **The Roundtrip Law, on demand**: verify any document JSON against a fresh
/// engine and answer with the summary comparison.
///
/// The window can call this on the JSON it is holding to show the law's result —
/// the same function the save path runs before it writes a byte.
#[tauri::command]
fn verify_document(document_json: String) -> Result<DocumentCheck, String> {
    let outcome = verify::verify(&document_json).map_err(|error| error.to_string())?;
    // Debug builds narrate what the law found — this is the line a headless run
    // reads to know the whole file path (engine → JSON → gzip → replay →
    // compare) works inside the window, not just in the test suite.
    #[cfg(debug_assertions)]
    println!(
        "<< verify_document: matches={} commands={} nodes={} variables={} — {}",
        outcome.matches, outcome.commands, outcome.nodes, outcome.variables, outcome.headline
    );
    Ok(outcome)
}

/// File ▸ New: forget where the last document lived, so the next Save asks.
#[tauri::command]
fn forget_path(state: State<'_, Host>) -> Result<(), String> {
    state.set_path(None)
}

/// Where the current document is, for the file bar.
#[tauri::command]
fn current_path(state: State<'_, Host>) -> Result<Option<String>, String> {
    Ok(state.path()?.map(|path| path.display().to_string()))
}

fn pick_save_target(
    app: &tauri::AppHandle,
    suggested_name: Option<String>,
) -> Result<Option<PathBuf>, String> {
    let name = suggested_name.unwrap_or_else(|| format!("untitled.{}", vectra_file::EXTENSION));
    file_io::ask_save(app, &name).map_err(|error| error.to_string())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .manage(Host {
            current_path: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            host_info,
            open_document,
            save_document,
            save_document_as,
            verify_document,
            forget_path,
            current_path,
        ])
        .on_page_load(|_webview, payload| {
            // Debug builds narrate the boot: which URL the window actually
            // loaded. (On a machine with no display this is the only way to see
            // how far a launch got, and it is how the headless run in
            // TASK-10.0-REPORT.md was diagnosed.) Release builds stay quiet —
            // hence the `let _` arm, so neither profile is left with an unused
            // binding.
            #[cfg(debug_assertions)]
            println!("<< the webview loaded {}", payload.url());
            #[cfg(not(debug_assertions))]
            let _ = payload;
        })
        .setup(|app| {
            let window = app
                .get_webview_window("main")
                .expect("tauri.conf.json declares a window named `main`");
            let _ = window.set_title("Vectra — untitled.vectra");
            println!(
                "vectra-desktop {} · {} {} · .vectra v{} · engine: in-window (the host verifies \
                 files with a short-lived engine)",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::OS,
                std::env::consts::ARCH,
                vectra_file::FORMAT_VERSION
            );
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running the Vectra desktop host");
}
