//! The container: header, gzip, and the errors a corrupt file can produce.

use std::io::{Read, Write};

use flate2::read::GzDecoder;
use flate2::{Compression, GzBuilder};

/// The six bytes every `.vectra` file starts with.
pub const MAGIC: &[u8; 6] = b"VECTRA";
/// The container version this build writes. Bumped only for a *breaking*
/// change; a reader that does not know a version refuses the file loudly rather
/// than guessing at its payload.
pub const FORMAT_VERSION: u16 = 1;
/// `MAGIC` plus the two version bytes.
pub const HEADER_LEN: usize = MAGIC.len() + 2;
/// The gzip stream's own magic — at offset [`HEADER_LEN`] in a valid file.
pub const COMPRESSED_MAGIC: [u8; 2] = [0x1f, 0x8b];
/// The extension, without the dot. The desktop app filters on it; nothing else
/// in the codebase should spell it out.
pub const EXTENSION: &str = "vectra";
/// Decompressed payload ceiling. A document is a few kilobytes of JSON; a
/// hundred megabytes of "document" is a decompression bomb, and the honest
/// answer to one is a typed refusal rather than an OOM.
pub const MAX_DECOMPRESSED_BYTES: usize = 64 * 1024 * 1024;

/// Everything that can go wrong reading or writing a `.vectra` file.
///
/// Typed, so the desktop layer can show a sentence and a test can assert on the
/// *kind* of failure rather than on a message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DocError {
    #[error("not a .vectra file (the {MAGIC_LEN} bytes `{found}` are not `VECTRA`)", MAGIC_LEN = MAGIC.len())]
    NotAVectraFile { found: String },

    #[error("this file is .vectra format version {found}; this build reads version {supported}")]
    UnsupportedVersion { found: u16, supported: u16 },

    #[error("the file ends after {found} byte(s); a .vectra header needs {HEADER_LEN}")]
    Truncated { found: usize },

    #[error("the payload is not a gzip stream (expected {expected:02x?}, found {found:02x?})")]
    NotCompressed { expected: [u8; 2], found: Vec<u8> },

    #[error("the payload is corrupt: {detail}")]
    Corrupt { detail: String },

    #[error("the document is {bytes} bytes decompressed, over the {limit}-byte ceiling")]
    TooLarge { bytes: usize, limit: usize },

    #[error("the payload is not valid UTF-8: {detail}")]
    NotUtf8 { detail: String },

    #[error("the payload is not valid JSON: {detail}")]
    NotJson { detail: String },
}

/// Encode a `Document` JSON string as `.vectra` bytes.
///
/// The JSON is validated **before** anything is compressed, so this function
/// cannot produce a file that a reader will reject as garbage. The gzip header
/// is pinned (mtime 0, OS byte 255) so the same document always produces the
/// same bytes — a determinism the tests rely on and a diff of two saves can.
pub fn encode(document_json: &str) -> Result<Vec<u8>, DocError> {
    validate_json(document_json)?;
    let mut encoder = GzBuilder::new()
        .mtime(0)
        .operating_system(255)
        .write(Vec::new(), Compression::default());
    encoder
        .write_all(document_json.as_bytes())
        .map_err(|error| DocError::Corrupt {
            detail: error.to_string(),
        })?;
    let payload = encoder.finish().map_err(|error| DocError::Corrupt {
        detail: error.to_string(),
    })?;
    if payload.len() < COMPRESSED_MAGIC.len() || payload[..2] != COMPRESSED_MAGIC {
        return Err(DocError::Corrupt {
            detail: "the gzip encoder did not emit a gzip stream".to_string(),
        });
    }

    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&payload);
    Ok(out)
}

/// Decode `.vectra` bytes back into the `Document` JSON string.
pub fn decode(bytes: &[u8]) -> Result<String, DocError> {
    let version = version_of(bytes)?;
    if version != FORMAT_VERSION {
        return Err(DocError::UnsupportedVersion {
            found: version,
            supported: FORMAT_VERSION,
        });
    }
    let payload = &bytes[HEADER_LEN..];
    if payload.len() < 2 || payload[..2] != COMPRESSED_MAGIC {
        return Err(DocError::NotCompressed {
            expected: COMPRESSED_MAGIC,
            found: payload.iter().take(2).copied().collect(),
        });
    }

    // The ceiling is enforced *while* inflating, so a bomb never materialises.
    let mut json = Vec::new();
    let mut limited = GzDecoder::new(payload).take(MAX_DECOMPRESSED_BYTES as u64 + 1);
    limited
        .read_to_end(&mut json)
        .map_err(|error| DocError::Corrupt {
            detail: error.to_string(),
        })?;
    if json.len() > MAX_DECOMPRESSED_BYTES {
        return Err(DocError::TooLarge {
            bytes: json.len(),
            limit: MAX_DECOMPRESSED_BYTES,
        });
    }

    let json = String::from_utf8(json).map_err(|error| DocError::NotUtf8 {
        detail: error.to_string(),
    })?;
    validate_json(&json)?;
    Ok(json)
}

/// The container version, read from the header **without inflating anything**.
///
/// This is what makes a version bump honest: an older build can say "this file
/// is version 2, I read version 1" instead of failing as if the file were
/// damaged.
pub fn version_of(bytes: &[u8]) -> Result<u16, DocError> {
    if bytes.len() < HEADER_LEN {
        return Err(DocError::Truncated { found: bytes.len() });
    }
    if &bytes[..MAGIC.len()] != MAGIC {
        return Err(DocError::NotAVectraFile {
            found: bytes[..MAGIC.len()]
                .iter()
                .map(|byte| {
                    if byte.is_ascii_graphic() {
                        char::from(*byte)
                    } else {
                        '.'
                    }
                })
                .collect(),
        });
    }
    Ok(u16::from_le_bytes([
        bytes[MAGIC.len()],
        bytes[MAGIC.len() + 1],
    ]))
}

/// Does this look like a `.vectra` file at all? (Header only — cheap enough for
/// a drag-and-drop pre-flight.)
pub fn is_vectra(bytes: &[u8]) -> bool {
    version_of(bytes).is_ok()
}

fn validate_json(json: &str) -> Result<(), DocError> {
    serde_json::from_str::<serde_json::Value>(json)
        .map(|_| ())
        .map_err(|error| DocError::NotJson {
            detail: error.to_string(),
        })
}

/// The size a `.vectra` payload would take, without keeping it: used by the
/// report the desktop layer logs after a save.
pub fn encoded_len(document_json: &str) -> Result<usize, DocError> {
    encode(document_json).map(|bytes| bytes.len())
}
