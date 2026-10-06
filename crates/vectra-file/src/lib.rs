//! **`vectra-file`** — the `.vectra` document container (Task 10.0 RULE 3).
//!
//! A `.vectra` file is a self-describing container whose payload is the
//! `Document` JSON, gzipped:
//!
//! ```text
//!   bytes 0..6   b"VECTRA"
//!   bytes 6..8   format version, u16 little-endian
//!   bytes 8..    an RFC 1952 gzip stream whose payload is UTF-8 JSON
//! ```
//!
//! Three properties this crate exists to make true:
//!
//! 1. **The format lives in Rust, not in JavaScript.** The frontend moves a
//!    `String` in and bytes out; it never learns the magic, the version, gzip,
//!    or the JSON's shape. A WebView that has never heard of deflate cannot
//!    corrupt a document.
//! 2. **A file is a *document*, not a screenshot.** The payload is the engine's
//!    own `Document` — parameters keep their parametric sources (`$base * 2` is
//!    still an expression after a save/load cycle), which is the whole point of
//!    the product.
//! 3. **Reopening rebuilds through the command boundary** ([`plan`]). A file
//!    that a hand-edit or a future version contradicts is refused by the
//!    engine's own gates when it is replayed, because it is replayed as
//!    *commands* — never by writing `Document` fields behind the engine's back.
//!
//! None of the engine crates know this crate exists; it depends on
//! `vectra-core` for the two types it must speak in (`Document`, `Command`).

pub mod format;
pub mod plan;

pub use format::{
    decode, encode, is_vectra, version_of, DocError, COMPRESSED_MAGIC, EXTENSION, FORMAT_VERSION,
    HEADER_LEN, MAGIC, MAX_DECOMPRESSED_BYTES,
};
pub use plan::{commands_to_json, document_from_json, document_to_commands, PlanReport};
