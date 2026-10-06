//! Native file I/O for the desktop host (Task 10.0 RULE 2 + RULE 3).
//!
//! * **RULE 2** — the dialog is the operating system's: `tauri_plugin_dialog`
//!   opens the platform picker, and the WebView never sees an
//!   `<input type="file">`, a `FileReader` or a `Blob`. A picked path is added to
//!   the fs plugin's scope *because it was picked*, which is the only reason a
//!   native app is allowed to touch it.
//! * **RULE 3** — the bytes on disk are a `.vectra` container: a `VECTRA` header
//!   over a gzip stream of the `Document` JSON. Compression and decompression
//!   happen **here**, in Rust (`vectra-file`); the frontend hands over a `String`
//!   and never learns what a deflate block is.
//!
//! Ordering note, and it is not cosmetic: a **save** encodes and verifies
//! *before* it writes, so a document the engine cannot reproduce never reaches
//! the disk and can never replace a good file with a bad one. An **open** reads
//! and verifies before it answers, so a file the engine refuses never reaches
//! the window.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

use vectra_file::format::{decode, encode, EXTENSION, FORMAT_VERSION, MAX_DECOMPRESSED_BYTES};
use vectra_file::DocError;

use crate::verify;

/// A document in hand: its JSON and where it wants to live.
pub struct DocumentFile {
    pub json: String,
    pub path: PathBuf,
}

/// One section that disagreed between a file and its replay.
#[derive(Debug, Clone, Serialize)]
pub struct SummaryDiff {
    pub field: String,
    pub expected: String,
    pub found: String,
}

/// The Roundtrip Law's answer for a document, as data the UI can render.
#[derive(Debug, Clone, Serialize)]
pub struct DocumentCheck {
    /// **The law**: `true` when save → close → reopen reproduced the document.
    pub matches: bool,
    /// How many commands the replay took.
    pub commands: usize,
    /// `42 command(s): 3 node(s), 1 variable(s), …` — the plan's own words.
    pub headline: String,
    /// The replay plan, so the window can rebuild the document in its engine.
    pub plan: Vec<Value>,
    pub nodes: usize,
    pub variables: usize,
    pub expressions: usize,
    pub constraints: usize,
    pub operations: usize,
    pub tracks: usize,
    pub procedural: usize,
    /// The engine's summary text for this document (what a prompt would carry).
    pub summary: String,
    /// Empty when `matches`; otherwise the sections that disagree.
    pub diffs: Vec<SummaryDiff>,
}

/// What `save_document` hands back — the write, and the proof.
#[derive(Debug, Clone, Serialize)]
pub struct SaveReport {
    pub path: String,
    /// Bytes written (the container, gzipped).
    pub bytes: usize,
    /// Bytes of JSON inside it — the number the status line can brag about.
    pub json_bytes: usize,
    /// Compression ratio, so the win is visible: `bytes / json_bytes`.
    pub ratio: f64,
    pub format_version: u16,
    /// `true` when the file already existed and was replaced.
    pub overwrote: bool,
    /// The Roundtrip Law, already applied to what was written.
    pub roundtrip: bool,
    /// The document's shape, in the plan's own words.
    pub headline: String,
}

/// What `open_document` hands back.
#[derive(Debug, Clone, Serialize)]
pub struct OpenReport {
    pub path: String,
    /// The document, as JSON. The window hands this to its engine as *commands*
    /// (`plan`), never as state.
    pub json: String,
    pub plan: Vec<Value>,
    pub headline: String,
    pub nodes: usize,
    pub variables: usize,
    pub format_version: u16,
    pub bytes: usize,
    pub summary: String,
}

/// Write a document as a `.vectra` file, then report the round trip.
///
/// The verification happens *first*. If the engine cannot reproduce this
/// document, nothing is written and the previous file survives untouched —
/// "never lose the work the user has" is worth one extra engine construction.
pub fn write_document(file: &DocumentFile) -> Result<SaveReport, String> {
    let check = verify::verify(&file.json).map_err(display)?;
    if !check.matches {
        return Err(refusal(&check));
    }
    // Encode: validate the payload, pin the gzip header, compress.
    let bytes = encode(&file.json).map_err(display)?;
    // …then inflate it again and compare, before anything touches the disk.
    // This is the byte-level half of the law — the replay check above proves the
    // *document* survives the round trip, this proves the *file* does, so the
    // bytes that land on disk are known to contain exactly the JSON the engine
    // produced. It costs one inflate of a few kilobytes; a save is not a hot path.
    let restored = decode(&bytes).map_err(display)?;
    if restored != file.json {
        return Err(
            "refusing to save: the compressed form did not give back the same document".to_string(),
        );
    }
    let overwrote = file.path.exists();

    // Write a sibling temp file, then rename over the target: an interrupted
    // write leaves the old file intact and never leaves a half-file behind.
    let temp = sibling_temp(&file.path);
    std::fs::write(&temp, &bytes).map_err(|error| {
        let _ = std::fs::remove_file(&temp);
        format!("could not write the file: {error}")
    })?;
    std::fs::rename(&temp, &file.path).map_err(|error| {
        let _ = std::fs::remove_file(&temp);
        format!("could not finish the save: {error}")
    })?;

    Ok(SaveReport {
        path: file.path.display().to_string(),
        bytes: bytes.len(),
        json_bytes: file.json.len(),
        ratio: ratio(bytes.len(), file.json.len()),
        format_version: FORMAT_VERSION,
        overwrote,
        roundtrip: check.matches,
        headline: check.headline,
    })
}

/// Read, inflate and verify a `.vectra` file.
pub fn read_document(path: &Path) -> Result<(DocumentFile, OpenReport), String> {
    let bytes = std::fs::read(path).map_err(|error| {
        format!(
            "could not read {}: {error}",
            path.file_name().unwrap_or_default().to_string_lossy()
        )
    })?;
    // Every failure between here and the report is a *sentence*: which part of
    // the file is wrong, and what the reader did about it.
    if bytes.len() as usize > MAX_DECOMPRESSED_BYTES {
        return Err(format!(
            "{} is {} bytes, which is larger than this build reads ({})",
            path.display(),
            bytes.len(),
            MAX_DECOMPRESSED_BYTES
        ));
    }
    let json = decode(&bytes).map_err(|error| match error {
        DocError::NotAVectraFile { .. } => format!(
            "{} is not a Vectra document (it does not start with the VECTRA header)",
            path.display()
        ),
        other => format!("{} could not be opened: {other}", path.display()),
    })?;
    let check = verify::verify(&json).map_err(display)?;
    if !check.matches {
        return Err(refusal(&check));
    }
    let report = OpenReport {
        path: path.display().to_string(),
        json: json.clone(),
        plan: check.plan.clone(),
        headline: check.headline.clone(),
        nodes: check.nodes,
        variables: check.variables,
        format_version: FORMAT_VERSION,
        bytes: bytes.len(),
        summary: check.summary.clone(),
    };
    Ok((
        DocumentFile {
            json,
            path: path.to_path_buf(),
        },
        report,
    ))
}

/// RULE 2: ask the OS for a `.vectra` file.
pub fn ask_open(app: &AppHandle) -> Result<Option<PathBuf>, DocError> {
    let picked = app
        .dialog()
        .file()
        .add_filter("Vectra document", &[EXTENSION])
        .add_filter("All files", &["*"])
        .set_title("Open a Vectra document")
        .blocking_pick_file();
    Ok(picked.and_then(|path| path.into_path().ok()))
}

/// RULE 2: ask the OS where to put one.
pub fn ask_save(app: &AppHandle, suggested: &str) -> Result<Option<PathBuf>, DocError> {
    let picked = app
        .dialog()
        .file()
        .add_filter("Vectra document", &[EXTENSION])
        .set_file_name(suggested)
        .set_title("Save the Vectra document")
        .blocking_save_file();
    Ok(picked.and_then(|path| path.into_path().ok()))
}

/// A save that refused, explained in terms of the section that disagreed.
fn refusal(check: &DocumentCheck) -> String {
    let detail = check
        .diffs
        .first()
        .map(|diff| {
            format!(
                "its `{}` section changed: expected {}, found {}",
                diff.field, diff.expected, diff.found
            )
        })
        .unwrap_or_else(|| "the replay produced a different document".to_string());
    format!(
        "refusing this document: the engine could not reproduce it from its own commands — {detail}"
    )
}

/// `sibling.vectra.tmp` beside the target (same filesystem, so `rename` is atomic).
fn sibling_temp(path: &Path) -> PathBuf {
    let mut temp = path.to_path_buf();
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| EXTENSION.to_string());
    temp.set_file_name(format!(".{name}.tmp"));
    temp
}

fn ratio(bytes: usize, json_bytes: usize) -> f64 {
    if json_bytes == 0 {
        return 1.0;
    }
    bytes as f64 / json_bytes as f64
}

/// `DocError` explains itself well; the frontend shows the sentence as-is.
fn display(error: DocError) -> String {
    error.to_string()
}
