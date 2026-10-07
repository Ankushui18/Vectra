//! **Text** (Task 11.0): the font library, shaping, layout and text-on-path.
//!
//! ```text
//!   NodeKind::Text ──▶ layout_text / layout_text_on_path ──▶ EvaluatedText
//!    (parameters)            (this module)                glyph outlines in
//!                                                          document space
//! ```
//!
//! # Why text is a *geometry* problem here
//!
//! The scene hands the renderer [`lyon::path::Path`]s; text joins that contract
//! by being outlined **once**, at evaluation time, so nothing downstream — the
//! tessellator, the hit test, the boolean engine, the exporters — ever learns
//! what a glyph is. A run is a path with knobs.
//!
//! # The two halves
//!
//! * **Shaping** ([`rustybuzz`], a HarfBuzz port) applies the face's own GSUB /
//!   GPOS tables: kerning, ligatures, mark positioning. The unit of work is a
//!   *line*, because that is what a shaper shapes.
//! * **Layout** is ours: where each shaped glyph goes (baseline, line advance,
//!   alignment), and — for a run bound to a curve — where the curve's
//!   arc length puts it. The output is spelled in the document's coordinate
//!   space (y down), not the font's (y up), so every consumer can forget which
//!   one it is looking at.
//!
//! # Fonts
//!
//! One face is **bundled** ([`BUNDLED_FACE`], a subset of DejaVu Sans renamed
//! to `Vectra Sans` — see `assets/LICENSE-VectraSans.txt`): a document always
//! draws its words, on any host, with no download and no system font. A host
//! may register more faces ([`FontLibrary`]); a family nobody has falls back to
//! the bundled one **and says so** (the evaluator turns that into a diagnostic),
//! because a silently substituted typeface is worse than a reported one.

use crate::paths::path_to_rings;
use crate::scene::{EvaluatedGlyph, EvaluatedText, TextMetrics};
use lyon::path::iterator::PathIterator;
use lyon::path::Path;
use std::collections::BTreeMap;
use std::f64::consts::PI;
use ttf_parser::OutlineBuilder;
use vectra_core::{FontProvider, Parameter, PathSegment, Point2, TextAlign};

/// The family name the bundled face answers to.
pub const BUNDLED_FAMILY: &str = vectra_core::DEFAULT_FONT_FAMILY;

/// The face every document can rely on: `Vectra Sans`, a subset of DejaVu Sans
/// (BITSTREAM VERA licence — `assets/LICENSE-VectraSans.txt`). Covers ASCII,
/// Latin-1 and the punctuation a designer actually sets, at 55 KB.
pub const BUNDLED_FACE: &[u8] = include_bytes!("../assets/vectra-sans.ttf");

/// Family names that resolve to the bundled face without any registration —
/// the generic families every design tool answers, so a document written
/// against `sans-serif` still draws on a host with no font library.
pub const BUNDLED_ALIASES: [&str; 5] = [
    "vectra sans",
    "dejavu sans",
    "sans-serif",
    "sans serif",
    "system-ui",
];

/// FNV-1a over a byte slice — the **face identity** the shaping cache keys on.
///
/// `const fn`, so the bundled face's id is computed by the compiler: no lazy
/// cell, no first-use cost, no way for the id to disagree with the bytes.
const fn face_fingerprint(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut index = 0;
    while index < bytes.len() {
        hash ^= bytes[index] as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
        index += 1;
    }
    hash
}

/// The identity of the **bundled** face — a compile-time constant.
pub const BUNDLED_FACE_ID: u64 = face_fingerprint(BUNDLED_FACE);

/// The face a run will be shaped with, plus the identity the shaping cache keys
/// on.
///
/// The two travel together because they must never disagree: `bytes` is what
/// rustybuzz and ttf-parser read; `id` is what a memo of the *shaped* run
/// remembers about those bytes. Keying such a memo on the family name would be
/// wrong the moment a host re-registered a face under a name it had used before
/// (the memo would serve stale outlines); keying it on a hash computed per frame
/// would be honest but wasteful. A provider that owns its bytes names them once
/// ([`FontProvider::face_id`]); anything else is hashed on the way in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FaceRef<'a> {
    pub bytes: &'a [u8],
    pub id: u64,
}

impl<'a> FaceRef<'a> {
    pub fn new(bytes: &'a [u8], id: u64) -> Self {
        Self { bytes, id }
    }

    /// The bundled face, with its identity.
    pub fn bundled() -> Self {
        Self::new(BUNDLED_FACE, BUNDLED_FACE_ID)
    }

    /// True ⟺ these are the bundled bytes.
    pub fn is_bundled(&self) -> bool {
        self.id == BUNDLED_FACE_ID
    }
}

/// Curve flattening tolerance for path sampling, in document units. The same
/// tolerance the renderer flattens at, so a run bound to a curve rides the
/// polyline the tessellator actually draws rather than a second approximation
/// of it — the difference is invisible at 0.05 units and fatal at 5.
pub const PATH_TOLERANCE: f32 = 0.05;

/// Flattening tolerance for **outlined** letterforms (RULE 3), in document
/// units. Coarser than [`PATH_TOLERANCE`] on purpose: an outline is *authored
/// geometry* — it becomes nodes the designer pushes, pulls and booleans — so a
/// vertex every 0.1 units is already far finer than any screen, and a compact
/// segment list keeps an outlined word a workable document.
pub const OUTLINE_TOLERANCE: f32 = 0.1;

/// Everything that can go wrong while laying out a run.
///
/// Total by design: empty text, an empty line and a degenerate path all produce
/// an *empty run*, not an error — the states a text node passes through while
/// it is being typed or bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextError {
    /// The bytes are not a font `ttf-parser` can read.
    InvalidFace(String),
    /// The face declares zero units per em, so no size is meaningful.
    DegenerateFace,
}

impl std::fmt::Display for TextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidFace(message) => write!(f, "invalid font face: {message}"),
            Self::DegenerateFace => write!(f, "the face declares zero units per em"),
        }
    }
}

impl std::error::Error for TextError {}

// ── the font library ────────────────────────────────────────────────────────

/// A host's faces, keyed by **normalized** family name, each keeping the
/// **spelled** name it was registered under.
///
/// Registration validates: bytes that do not parse are refused and nothing
/// changes, so a bad upload cannot become a text node that silently draws
/// nothing. Resolution is case- and space-insensitive (`"Vectra Sans"` and
/// `"  vectra   sans"` are one family), because a family name is a label a
/// designer types, not a key that has to match byte for byte.
///
/// The two names are kept apart on purpose: the normalized key is the lookup,
/// and the spelled one is what a **font picker** must show. A picker offering
/// `vectra sans` because that is the internal key would be renaming the
/// designer's typeface in a menu.
#[derive(Debug, Clone, Default)]
pub struct FontLibrary {
    faces: BTreeMap<String, Face>,
}

#[derive(Debug, Clone)]
struct Face {
    /// The family as it was registered — what a menu shows.
    display: String,
    bytes: Vec<u8>,
    /// Identity of `bytes`, computed **once** when the face arrives: this is
    /// what the shaping cache keys on, so it must change whenever the bytes do.
    id: u64,
}

impl FontLibrary {
    /// An empty library: every family resolves to the bundled face.
    pub fn new() -> Self {
        Self::default()
    }

    /// A library holding the bundled face (and its aliases), so `families()`
    /// and `face()` answer for it exactly as they would for a registered one.
    ///
    /// The aliases are registered as spellings of the same face rather than as
    /// separate families: `sans-serif` and `system-ui` must *resolve*, and must
    /// not each appear in the picker as if they were two more typefaces.
    pub fn bundled() -> Self {
        let mut library = Self::new();
        library.faces.insert(
            normalize(BUNDLED_FAMILY),
            Face {
                display: BUNDLED_FAMILY.to_string(),
                bytes: BUNDLED_FACE.to_vec(),
                id: BUNDLED_FACE_ID,
            },
        );
        for alias in BUNDLED_ALIASES {
            library.faces.insert(
                normalize(alias),
                Face {
                    display: BUNDLED_FAMILY.to_string(),
                    bytes: BUNDLED_FACE.to_vec(),
                    id: BUNDLED_FACE_ID,
                },
            );
        }
        library
    }

    /// Register (or replace) a face. Validating — a face that does not parse is
    /// refused with the reason, and the library is untouched.
    pub fn register(&mut self, family: &str, bytes: Vec<u8>) -> Result<(), TextError> {
        let key = normalize(family);
        if key.is_empty() {
            return Err(TextError::InvalidFace("empty family name".to_string()));
        }
        ttf_parser::Face::parse(&bytes, 0)
            .map_err(|error| TextError::InvalidFace(error.to_string()))?;
        let display = family.split_whitespace().collect::<Vec<_>>().join(" ");
        let id = face_fingerprint(&bytes);
        self.faces.insert(key, Face { display, bytes, id });
        Ok(())
    }

    /// Every registered family, sorted, **de-duplicated by face**: the
    /// spellings a picker must show, with each face named once (its aliases are
    /// alternatives to type, not typefaces of their own).
    pub fn families(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .faces
            .values()
            .map(|face| face.display.clone())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// The bytes for a family, if this library has them (no fallback).
    pub fn face_bytes(&self, family: &str) -> Option<&[u8]> {
        self.faces
            .get(&normalize(family))
            .map(|face| face.bytes.as_slice())
    }

    /// The identity of a family's bytes — see [`FaceRef`].
    pub fn face_id(&self, family: &str) -> Option<u64> {
        self.faces.get(&normalize(family)).map(|face| face.id)
    }

    pub fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }
}

impl FontProvider for FontLibrary {
    fn face(&self, family: &str) -> Option<&[u8]> {
        self.face_bytes(family)
    }

    fn families(&self) -> Vec<String> {
        FontLibrary::families(self)
    }

    fn face_id(&self, family: &str) -> Option<u64> {
        FontLibrary::face_id(self, family)
    }
}

/// Lowercase, collapse whitespace: the identity of a family name.
fn normalize(family: &str) -> String {
    family
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The face a run shapes with, plus whether a **fallback** happened.
///
/// Resolution order: the host's library, then the bundled face. The second
/// element is `true` exactly when the family is neither registered nor one of
/// the bundled aliases — the condition the evaluator reports as a diagnostic,
/// so the designer learns their font is not on this machine while the artwork
/// still renders.
///
/// The returned [`FaceRef`] carries the identity the shaping cache keys on:
/// the provider's own id when it has one, the bundled constant for the bundled
/// face, and — for a provider that cannot name its faces — a fingerprint of the
/// bytes, computed here and now. That last case is the only one that costs
/// anything, and it costs a hash of the face, not a reshape of the run.
pub fn resolve_face<'a>(fonts: Option<&'a dyn FontProvider>, family: &str) -> (FaceRef<'a>, bool) {
    if let Some(provider) = fonts {
        if let Some(bytes) = provider.face(family) {
            let id = provider
                .face_id(family)
                .unwrap_or_else(|| face_fingerprint(bytes));
            return (FaceRef::new(bytes, id), false);
        }
    }
    let bundled = BUNDLED_ALIASES.contains(&normalize(family).as_str());
    (FaceRef::bundled(), !bundled)
}

// ── the request ─────────────────────────────────────────────────────────────

/// A run's resolved typographic inputs — what the evaluator hands the layout
/// engine once every [`Parameter`] has resolved.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextSpec<'a> {
    pub text: &'a str,
    pub family: &'a str,
    /// Em size in document units (`font_size`).
    pub size: f64,
    /// Extra advance per glyph, in document units (negative tightens).
    pub letter_spacing: f64,
    /// Baseline-to-baseline distance as a **multiple of `size`** — CSS's
    /// reading of a line-height, so animating the size scales the leading with
    /// it instead of collapsing the lines together.
    pub line_height: f64,
    pub alignment: TextAlign,
}

impl<'a> TextSpec<'a> {
    pub fn new(text: &'a str, family: &'a str, size: f64) -> Self {
        Self {
            text,
            family,
            size,
            letter_spacing: 0.0,
            line_height: vectra_core::DEFAULT_LINE_HEIGHT,
            alignment: TextAlign::Left,
        }
    }
}

// ── affine transform (font space → document space) ──────────────────────────

/// A 2-D affine transform, spelled `x' = a·x + c·y + e`, `y' = b·x + d·y + f`.
///
/// The layout needs exactly two of these: the **straight** one (scale, flip the
/// font's y-up axis into the document's y-down one, translate to the pen) and
/// the **on-path** one (the same, then a rotation onto the tangent). Writing
/// them once, as data, keeps the per-glyph loop honest — there is no place for a
/// stray sign to hide.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Affine {
    pub const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    pub fn translate(dx: f64, dy: f64) -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: dx,
            f: dy,
        }
    }

    /// Scale, with independent axes — the y factor is negative when flipping
    /// font space into document space.
    pub fn scale(sx: f64, sy: f64) -> Self {
        Self {
            a: sx,
            b: 0.0,
            c: 0.0,
            d: sy,
            e: 0.0,
            f: 0.0,
        }
    }

    pub fn rotate(radians: f64) -> Self {
        let (sin, cos) = radians.sin_cos();
        Self {
            a: cos,
            b: sin,
            c: -sin,
            d: cos,
            e: 0.0,
            f: 0.0,
        }
    }

    /// Apply `self` first, then `next` — `next ∘ self`.
    pub fn then(self, next: &Affine) -> Self {
        Self {
            a: next.a * self.a + next.c * self.b,
            b: next.b * self.a + next.d * self.b,
            c: next.a * self.c + next.c * self.d,
            d: next.b * self.c + next.d * self.d,
            e: next.a * self.e + next.c * self.f + next.e,
            f: next.b * self.e + next.d * self.f + next.f,
        }
    }

    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    /// **Font space → document space** for a glyph on a straight baseline.
    pub fn straight(scale: f64, pen_x: f64, baseline_y: f64) -> Self {
        Self::scale(scale, -scale).then(&Self::translate(pen_x, baseline_y))
    }

    /// **Font space → document space** for a glyph riding a tangent: scale, flip
    /// y, rotate onto the tangent, land on the point.
    pub fn on_path(scale: f64, angle: f64, x: f64, y: f64) -> Self {
        Self::scale(scale, -scale)
            .then(&Self::rotate(angle))
            .then(&Self::translate(x, y))
    }
}

// ── outlines ────────────────────────────────────────────────────────────────

/// A [`ttf_parser::OutlineBuilder`] that emits a `lyon` path through an
/// [`Affine`] — the one bridge between the font's units and the document's.
struct GlyphOutline {
    builder: lyon::path::Builder,
    transform: Affine,
    open: bool,
}

impl GlyphOutline {
    fn new(transform: Affine) -> Self {
        Self {
            builder: Path::builder(),
            transform,
            open: false,
        }
    }

    fn point(&self, x: f32, y: f32) -> lyon::math::Point {
        let (x, y) = self.transform.apply(x as f64, y as f64);
        lyon::math::point(x as f32, y as f32)
    }

    fn finish(mut self) -> Path {
        if self.open {
            self.builder.end(false);
        }
        self.builder.build()
    }
}

impl OutlineBuilder for GlyphOutline {
    fn move_to(&mut self, x: f32, y: f32) {
        if self.open {
            self.builder.end(false);
            self.open = false;
        }
        let at = self.point(x, y);
        let _ = self.builder.begin(at);
        self.open = true;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        if !self.open {
            return;
        }
        let to = self.point(x, y);
        self.builder.line_to(to);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        if !self.open {
            return;
        }
        let ctrl = self.point(x1, y1);
        let to = self.point(x, y);
        self.builder.quadratic_bezier_to(ctrl, to);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        if !self.open {
            return;
        }
        let c1 = self.point(x1, y1);
        let c2 = self.point(x2, y2);
        let to = self.point(x, y);
        self.builder.cubic_bezier_to(c1, c2, to);
    }

    fn close(&mut self) {
        if self.open {
            self.builder.close();
            self.open = false;
        }
    }
}

/// `source`, mapped through `transform` — the **placement** of a cached glyph.
///
/// This is the whole per-frame cost of moving a run: the contours were built
/// once, in the run's own frame, and a font size, a string or a family change is
/// what invalidates them. An `offset` drag, a moving circle or a drag of the
/// text node re-runs exactly this function and nothing else.
fn transformed_path(source: &Path, transform: &Affine) -> Path {
    let mut builder = Path::builder();
    append_transformed(&mut builder, source, transform);
    builder.build()
}

/// `append_path`, with every point mapped through an [`Affine`] on the way in.
fn append_transformed(builder: &mut lyon::path::Builder, source: &Path, transform: &Affine) {
    use lyon::path::Event;
    let point = |p: lyon::math::Point| {
        let (x, y) = transform.apply(p.x as f64, p.y as f64);
        lyon::math::point(x as f32, y as f32)
    };
    let mut open = false;
    for event in source.iter() {
        match event {
            Event::Begin { at } => {
                if open {
                    builder.end(false);
                }
                let _ = builder.begin(point(at));
                open = true;
            }
            Event::Line { to, .. } => {
                builder.line_to(point(to));
            }
            Event::Quadratic { ctrl, to, .. } => {
                builder.quadratic_bezier_to(point(ctrl), point(to));
            }
            Event::Cubic {
                ctrl1, ctrl2, to, ..
            } => {
                builder.cubic_bezier_to(point(ctrl1), point(ctrl2), point(to));
            }
            Event::End { close, .. } => {
                if close {
                    builder.close();
                }
                open = false;
            }
        }
    }
    if open {
        builder.end(false);
    }
}

/// Append every event of `source` to `builder`, preserving closure — how a
/// run's per-glyph outlines become the one path the renderer fills.
fn append_path(builder: &mut lyon::path::Builder, source: &Path) {
    use lyon::path::Event;
    let mut open = false;
    for event in source.iter() {
        match event {
            Event::Begin { at } => {
                if open {
                    builder.end(false);
                }
                let _ = builder.begin(at);
                open = true;
            }
            Event::Line { to, .. } => {
                builder.line_to(to);
            }
            Event::Quadratic { ctrl, to, .. } => {
                builder.quadratic_bezier_to(ctrl, to);
            }
            Event::Cubic {
                ctrl1, ctrl2, to, ..
            } => {
                builder.cubic_bezier_to(ctrl1, ctrl2, to);
            }
            Event::End { close, .. } => {
                if close {
                    builder.close();
                }
                open = false;
            }
        }
    }
    if open {
        builder.end(false);
    }
}

/// One glyph's contours, through `transform`.
fn glyph_outline(face: &ttf_parser::Face<'_>, glyph_id: u16, transform: Affine) -> Path {
    let mut outline = GlyphOutline::new(transform);
    // An empty result is normal — a space has no contours — and must not fail
    // the run: the glyph still advances the pen.
    let _ = face.outline_glyph(ttf_parser::GlyphId(glyph_id), &mut outline);
    outline.finish()
}

// ── shaping ─────────────────────────────────────────────────────────────────

/// One shaped glyph, in document units and **font-space signs** (y up): the
/// layout decides where it lands, the shaper decides how wide it is and how it
/// sits relative to the pen.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ShapedGlyph {
    glyph_id: u16,
    /// Byte offset of the glyph's character in the run's string. Clusters are
    /// relative to the *line* the shaper saw; [`shape_line`] adds the line's
    /// offset so every glyph in a multi-line run is addressable absolutely.
    cluster: usize,
    advance: f64,
    offset_x: f64,
    offset_y: f64,
}

/// The metrics a run's box is built from, in document units.
#[derive(Debug, Clone, Copy, PartialEq)]
struct FaceMetrics {
    scale: f64,
    ascender: f64,
    descender: f64,
    line_advance: f64,
}

fn face_metrics(face: &ttf_parser::Face<'_>, size: f64, line_height: f64) -> FaceMetrics {
    let units_per_em = face.units_per_em() as f64;
    let scale = size / units_per_em;
    FaceMetrics {
        scale,
        ascender: face.ascender() as f64 * scale,
        descender: face.descender() as f64 * scale,
        line_advance: size * line_height,
    }
}

/// Shape one line with `rustybuzz`, in document units.
///
/// HarfBuzz does the hard part — GSUB substitutions, GPOS positioning, script
/// shaping — and this function's only job is to convert its numbers into ours
/// and to add `letter_spacing`, which is a *designer's* number and deliberately
/// not part of shaping.
fn shape_line(
    shaper: &rustybuzz::Face<'_>,
    line: &str,
    line_offset: usize,
    metrics: FaceMetrics,
    letter_spacing: f64,
) -> Vec<ShapedGlyph> {
    if line.is_empty() {
        return Vec::new();
    }
    let mut buffer = rustybuzz::UnicodeBuffer::new();
    buffer.push_str(line);
    let shaped = rustybuzz::shape(shaper, &[], buffer);
    let infos = shaped.glyph_infos();
    let positions = shaped.glyph_positions();
    infos
        .iter()
        .zip(positions.iter())
        .map(|(info, position)| ShapedGlyph {
            glyph_id: info.glyph_id as u16,
            cluster: info.cluster as usize + line_offset,
            advance: position.x_advance as f64 * metrics.scale + letter_spacing,
            offset_x: position.x_offset as f64 * metrics.scale,
            offset_y: position.y_offset as f64 * metrics.scale,
        })
        .collect()
}

/// Split a run into lines, keeping the `\n` semantics of a text editor: `""` is
/// one empty line (a cursor with nothing typed yet), `"a\n"` is two lines —
/// each with its byte offset inside the run, so a glyph's cluster is absolute
/// and the outline command can name a letterform after the character it draws.
fn split_lines(text: &str) -> Vec<(&str, usize)> {
    let mut lines = Vec::new();
    let mut offset = 0usize;
    for line in text.split('\n') {
        lines.push((line, offset));
        offset += line.len() + 1;
    }
    lines
}

/// The advance width of one shaped line, tracking included.
fn line_width(glyphs: &[ShapedGlyph]) -> f64 {
    glyphs.iter().map(|glyph| glyph.advance).sum()
}

/// A run that draws nothing but keeps the metrics a *shaped* run would have
/// had — the shape a bound run takes when its path has no direction yet.
fn empty_run_from(metrics: &TextMetrics) -> EvaluatedText {
    EvaluatedText {
        glyphs: Vec::new(),
        outline: Path::builder().build(),
        metrics: TextMetrics {
            width: 0.0,
            height: metrics.height,
            ascender: metrics.ascender,
            descender: metrics.descender,
            line_advance: metrics.line_advance,
            lines: metrics.lines,
        },
        truncated: 0,
    }
}

// ── the shaping cache ───────────────────────────────────────────────────────

/// One shaped glyph, in the run's **own frame**: its contours sit at the
/// origin, upright, before the run is placed anywhere.
///
/// This is the half of text that is expensive — rustybuzz shapes the line,
/// ttf-parser walks every contour — and the half that does **not** depend on
/// where the run goes. A `font_size` or a string changes it; an `offset`, a
/// `path_offset` drag or a moving circle does not.
#[derive(Debug, Clone)]
struct LocalGlyph {
    glyph_id: u16,
    cluster: usize,
    advance: f64,
    /// Distance from the run's anchor along its baseline — the arc length the
    /// glyph will occupy once the run is bound. Alignment is applied here;
    /// the path `offset` is **not**, because that is a placement, and a
    /// placement must not invalidate a shaped run.
    distance: f64,
    /// Which line the glyph belongs to: a bound run drops line `i` by its own
    /// line advance at placement time.
    line: usize,
    offset_x: f64,
    offset_y: f64,
    /// The glyph's contours in the run's own frame.
    outline: Path,
}

/// A shaped run: every glyph, plus the metrics that do not depend on the place.
#[derive(Debug, Clone)]
struct LocalRun {
    glyphs: Vec<LocalGlyph>,
    /// `width` is the widest line's advance width ([`layout_text_on_path`]
    /// overrides it with the path's arc length — a placement decision), and
    /// `height` is `(lines − 1) × line_advance + ascender − descender`.
    metrics: TextMetrics,
}

/// What a shaped run is a pure function of — everything else about a text node
/// is placement.
///
/// `size`, `letter_spacing` and `line_height` are compared as **bits**, not as
/// floats: the memo must answer "the same run?" and an f64 equality that
/// conflated `0.0` with `-0.0` would be a different question.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RunKey {
    face: u64,
    text: String,
    size_bits: u64,
    spacing_bits: u64,
    leading_bits: u64,
    alignment: &'static str,
}

impl RunKey {
    fn of(face: FaceRef<'_>, spec: &TextSpec<'_>) -> Self {
        Self {
            face: face.id,
            text: spec.text.to_string(),
            size_bits: spec.size.to_bits(),
            spacing_bits: spec.letter_spacing.to_bits(),
            leading_bits: spec.line_height.to_bits(),
            alignment: spec.alignment.tag(),
        }
    }
}

/// How many shaped runs one thread remembers. Each entry is a handful of
/// outlines — a word's worth of contours — so this is a few hundred kilobytes
/// at worst, and the eviction policy is plain LRU: the run a designer is
/// editing stays, the one they left stays until the cache fills.
const SHAPE_CACHE_CAPACITY: usize = 48;

#[derive(Debug, Default)]
struct ShapeCache {
    entries: Vec<(RunKey, std::sync::Arc<LocalRun>)>,
    hits: u64,
    misses: u64,
}

/// What the shaping cache has done on this thread: the numbers a performance
/// test reads, and the numbers a profiler would otherwise have to guess at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShapeCacheStats {
    pub entries: usize,
    pub hits: u64,
    pub misses: u64,
}

thread_local! {
    /// The shaping cache (RULE 1's performance half).
    ///
    /// Thread-local rather than global: shaping is CPU work with no shared
    /// state, a wasm engine is single-threaded, and a lock on the hot path of
    /// every text evaluation would cost more than the cache saves. Each thread
    /// warms its own.
    static SHAPE_CACHE: std::cell::RefCell<ShapeCache> =
        std::cell::RefCell::new(ShapeCache::default());
}

/// The shaping cache's counters (see [`ShapeCacheStats`]).
pub fn shape_cache_stats() -> ShapeCacheStats {
    SHAPE_CACHE.with(|cache| {
        let cache = cache.borrow();
        ShapeCacheStats {
            entries: cache.entries.len(),
            hits: cache.hits,
            misses: cache.misses,
        }
    })
}

/// Empty the shaping cache. Used by tests that want a cold start; a document
/// never needs it (the memo is keyed, so a stale entry is unreadable).
pub fn clear_shape_cache() {
    SHAPE_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.entries.clear();
        cache.hits = 0;
        cache.misses = 0;
    });
}

/// The shaped run for a face + spec, from the cache when possible.
///
/// A miss does the expensive work once ([`build_local_run`]) and remembers it;
/// a hit clones an `Arc` and a handful of `f64`s. Either way what comes back is
/// in the run's own frame, so the caller's only remaining job is placement —
/// which is the point: an `offset` slider drag re-places, it does not re-shape.
fn shaped_run(
    face: FaceRef<'_>,
    spec: &TextSpec<'_>,
) -> Result<std::sync::Arc<LocalRun>, TextError> {
    let key = RunKey::of(face, spec);
    SHAPE_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(index) = cache.entries.iter().position(|(k, _)| *k == key) {
            cache.hits += 1;
            // LRU: the entry just used is the last to be evicted.
            let entry = cache.entries.remove(index);
            let run = entry.1.clone();
            cache.entries.push(entry);
            return Ok(run);
        }
        let run = std::sync::Arc::new(build_local_run(face.bytes, spec)?);
        cache.misses += 1;
        if cache.entries.len() >= SHAPE_CACHE_CAPACITY {
            cache.entries.remove(0);
        }
        cache.entries.push((key, run.clone()));
        Ok(run)
    })
}

/// The expensive half of text — shaping and glyph outlining — done once per
/// distinct (face, run) pair, in the run's own frame.
fn build_local_run(face_bytes: &[u8], spec: &TextSpec<'_>) -> Result<LocalRun, TextError> {
    let face = ttf_parser::Face::parse(face_bytes, 0)
        .map_err(|error| TextError::InvalidFace(error.to_string()))?;
    if face.units_per_em() == 0 {
        return Err(TextError::DegenerateFace);
    }
    let shaper = rustybuzz::Face::from_slice(face_bytes, 0)
        .ok_or_else(|| TextError::InvalidFace("the shaper could not adopt the face".to_string()))?;
    let metrics = face_metrics(&face, spec.size, spec.line_height);

    let lines = split_lines(spec.text);
    let shaped: Vec<Vec<ShapedGlyph>> = lines
        .iter()
        .map(|(line, offset)| shape_line(&shaper, line, *offset, metrics, spec.letter_spacing))
        .collect();
    let widths: Vec<f64> = shaped.iter().map(|glyphs| line_width(glyphs)).collect();

    let mut glyphs: Vec<LocalGlyph> = Vec::new();
    for (line_index, line_glyphs) in shaped.iter().enumerate() {
        // Line 0's baseline is the run's anchor; later lines flow downward by
        // one line advance. The frame is the origin, so this is where the
        // glyphs sit *relative to the anchor*.
        let baseline = line_index as f64 * metrics.line_advance;
        let mut pen = spec.alignment.anchor_offset(widths[line_index]);
        for shaped_glyph in line_glyphs {
            let transform = Affine::straight(
                metrics.scale,
                pen + shaped_glyph.offset_x,
                baseline - shaped_glyph.offset_y,
            );
            glyphs.push(LocalGlyph {
                glyph_id: shaped_glyph.glyph_id,
                cluster: shaped_glyph.cluster,
                advance: shaped_glyph.advance,
                distance: pen,
                line: line_index,
                offset_x: shaped_glyph.offset_x,
                offset_y: shaped_glyph.offset_y,
                outline: glyph_outline(&face, shaped_glyph.glyph_id, transform),
            });
            pen += shaped_glyph.advance;
        }
    }

    let width = widths.iter().copied().fold(0.0f64, f64::max);
    let height = (lines.len() as f64 - 1.0).max(0.0) * metrics.line_advance + metrics.ascender
        - metrics.descender;
    Ok(LocalRun {
        glyphs,
        metrics: TextMetrics {
            width,
            height,
            ascender: metrics.ascender,
            descender: metrics.descender,
            line_advance: metrics.line_advance,
            lines: lines.len(),
        },
    })
}

// ── straight layout ─────────────────────────────────────────────────────────

/// Lay a run out on its own baselines (RULE 1).
///
/// `origin` is the node's `(x, y)`: the **first line's baseline start**, with
/// alignment applied about it ([`TextAlign::anchor_offset`], the one place
/// that decision is made). Later lines flow downward `size × line_height`
/// apart.
pub fn layout_text(
    face: FaceRef<'_>,
    spec: &TextSpec<'_>,
    origin: (f64, f64),
) -> Result<EvaluatedText, TextError> {
    let run = shaped_run(face, spec)?;
    // The straight placement is a **translation**: every cached outline is
    // already upright and anchored about its own origin, so putting the run at
    // `(x, y)` is one affine, applied per glyph. No shaping, no contour walk.
    let place = Affine::translate(origin.0, origin.1);
    let mut glyphs: Vec<EvaluatedGlyph> = Vec::with_capacity(run.glyphs.len());
    let mut outline = Path::builder();
    for local in &run.glyphs {
        let glyph = transformed_path(&local.outline, &place);
        append_path(&mut outline, &glyph);
        glyphs.push(EvaluatedGlyph {
            glyph_id: local.glyph_id,
            cluster: local.cluster,
            advance: local.advance,
            distance: local.distance,
            angle: 0.0,
            outline: glyph,
        });
    }

    Ok(EvaluatedText {
        glyphs,
        outline: outline.build(),
        metrics: run.metrics,
        // A straight run has no end to run off: every glyph it shaped is drawn.
        truncated: 0,
    })
}

// ── text on a path (RULE 2) ─────────────────────────────────────────────────

/// One sample of a flattened path: a point and how far along the path it sits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathSample {
    pub x: f64,
    pub y: f64,
    /// Arc length from the path's start to this point.
    pub length: f64,
}

/// The finest flattening [`sample_path`] will refine to, in document units.
/// Below this the samples are dense enough that no tangent question remains.
const MIN_PATH_TOLERANCE: f32 = 1e-4;

/// The largest direction change between adjacent samples a run will accept,
/// in radians (≈ 0.57°).
///
/// This is the number that makes text on a path look *set* rather than
/// segmented. A tangent read from a flattened path is accurate only to about
/// half the turn between two neighbouring samples, and that error is a
/// **discrete jump** the type inherits: at the default tolerance a circle of
/// radius 440 flattens into 43-unit chords, so neighbouring glyphs differ in
/// rotation by up to 5.6° from each other and from the true tangent — visible
/// as a word that kinks around the curve instead of following it.
const TANGENT_TURN: f64 = 0.01;

/// Flatten `path` into arc-length samples, **refined until a tangent read from
/// them is faithful**.
///
/// Repeated points are dropped, because they carry no direction — the sampler
/// below needs a *tangent*, and a zero-length segment has none.
///
/// # Why this refines itself
///
/// Flattening is normally asked one question — "how far does the polygon stray
/// from the curve?" — and a tolerance in document units answers it. Text on a
/// path asks a second one — "which way is the curve *going* here?" — and the
/// same tolerance answers that badly on a large, gently curved path, where long
/// chords are geometrically fine but directionally coarse (see [`TANGENT_TURN`]).
/// Refining only where it is needed keeps a straight logo line at its original
/// handful of samples, and spends its budget on the curves that need it.
pub fn sample_path(path: &Path) -> Vec<PathSample> {
    let mut tolerance = PATH_TOLERANCE;
    let mut samples = sample_path_at(path, tolerance);
    // Four refinements is a hard ceiling: each one shrinks the tolerance by at
    // least 20×, and past `MIN_PATH_TOLERANCE` there is nothing left to learn.
    for _ in 0..4 {
        let worst = worst_turn(&samples);
        if worst <= TANGENT_TURN || tolerance <= MIN_PATH_TOLERANCE {
            break;
        }
        // The chord length is `√(8·radius·tolerance)` and the turn is the chord
        // over the radius, so the turn scales with `√tolerance` — halving the
        // turn needs a quarter of the tolerance. The 0.8 is headroom: the worst
        // segment after a refinement is not the one that was worst before it.
        let ratio = ((TANGENT_TURN / worst).powi(2) * 0.8).clamp(0.02, 0.5);
        tolerance = (tolerance * ratio as f32).max(MIN_PATH_TOLERANCE);
        samples = sample_path_at(path, tolerance);
    }
    samples
}

/// The largest direction change between adjacent samples (radians).
fn worst_turn(samples: &[PathSample]) -> f64 {
    let mut worst: f64 = 0.0;
    for window in samples.windows(3) {
        let (a, b, c) = (window[0], window[1], window[2]);
        let first = tangent_between(a, b);
        let second = tangent_between(b, c);
        let mut delta = (second - first).abs();
        while delta > PI {
            delta = (delta - 2.0 * PI).abs();
        }
        worst = worst.max(delta);
    }
    if samples.len() < 3 {
        // Two samples cannot show a turn, but the caller still gets both ends.
        return 0.0;
    }
    worst
}

/// `sample_path`, at an explicit tolerance.
fn sample_path_at(path: &Path, tolerance: f32) -> Vec<PathSample> {
    let mut samples: Vec<PathSample> = Vec::new();
    let mut length = 0.0f64;
    for event in path.iter().flattened(tolerance) {
        let point = match event {
            lyon::path::PathEvent::Begin { at } => at,
            lyon::path::PathEvent::Line { to, .. } => to,
            lyon::path::PathEvent::End { last, .. } => last,
            _ => continue,
        };
        let (x, y) = (point.x as f64, point.y as f64);
        if let Some(previous) = samples.last() {
            let step = ((x - previous.x).powi(2) + (y - previous.y).powi(2)).sqrt();
            if step == 0.0 {
                continue;
            }
            length += step;
        }
        samples.push(PathSample { x, y, length });
    }
    samples
}

/// The total arc length of a sampled path.
pub fn path_length(samples: &[PathSample]) -> f64 {
    samples.last().map(|sample| sample.length).unwrap_or(0.0)
}

/// Where arc length `distance` lands on a sampled path: the point and the
/// **tangent angle** (radians, in document space).
///
/// The answer is **interpolated along the sample segment that contains
/// `distance`**, not rounded to the nearest sample. Rounding would quantize a
/// sliding run to the flattening grid, so a dragged `offset` would visibly step
/// instead of slide, and RULE 2's "slides smoothly" would be a claim about the
/// arithmetic rather than about what is on screen. The direction is the
/// segment's — the chord's — and [`sample_path`] refines the grid until a chord
/// is a faithful stand-in for the tangent.
///
/// Distances outside the path are not errors — a run may legitimately start
/// before the first sample (a negative `offset`) or overrun the end while a
/// slider is being dragged. The end answer is the last sample's own, position
/// *and* tangent, which keeps a bound run's motion continuous instead of
/// popping it in and out of existence at the ends. The same "clamp, don't skip"
/// policy the evaluator applies to every other mappable value.
pub fn sample_at(samples: &[PathSample], distance: f64) -> Option<(f64, f64, f64)> {
    let first = *samples.first()?;
    let last = *samples.last().expect("non-empty has a last");
    if distance <= first.length {
        let ahead = samples.get(1).copied().unwrap_or(first);
        return Some((first.x, first.y, tangent_between(first, ahead)));
    }
    if distance >= last.length {
        let behind = samples
            .get(samples.len().saturating_sub(2))
            .copied()
            .unwrap_or(last);
        return Some((last.x, last.y, tangent_between(behind, last)));
    }
    // `partition_point` gives the first sample *at or past* `distance`; the
    // segment that contains it is `[index - 1, index]`, and the clamps above
    // guarantee both ends exist.
    let index = samples
        .partition_point(|sample| sample.length <= distance)
        .clamp(1, samples.len() - 1);
    let from = samples[index - 1];
    let to = samples[index];
    let span = to.length - from.length;
    let t = if span > 0.0 {
        (distance - from.length) / span
    } else {
        0.0
    };
    Some((
        from.x + (to.x - from.x) * t,
        from.y + (to.y - from.y) * t,
        tangent_between(from, to),
    ))
}

/// The direction from one sample to the next; `0` for a zero-length step (only
/// possible on a degenerate path, where the run lies along the x axis and still
/// draws).
fn tangent_between(from: PathSample, to: PathSample) -> f64 {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    if dx == 0.0 && dy == 0.0 {
        0.0
    } else {
        dy.atan2(dx)
    }
}

/// A glyph's up direction after a rotation of `angle`, as a unit vector.
///
/// Font space has y **up**; the layout flips it into the document's y-down
/// space, so a glyph's up direction is the rotated image of `(0, -1)`. This is
/// the vector the readability pass tests: `up.y < 0` means "points up on
/// screen", which is the whole of "is this glyph upright?".
fn glyph_up(angle: f64) -> (f64, f64) {
    (angle.sin(), -angle.cos())
}

/// Where a bound glyph's pen lands: the path point, the multi-line drop toward
/// the glyph's own **down** direction, and the glyph's own attachment offset.
///
/// The offset is the shaper's (`offset_x`/`offset_y`, already in document units
/// and still carrying the font's y-up signs), and it is a vector in the glyph's
/// **unrotated** frame: it must be *rotated with the glyph*, not slid along the
/// tangent. Sliding it along the tangent is what a mark-attached accent would
/// look like if it were detached — the acute of an `é` set on a curve would
/// drift away from its `e` by exactly the angle the run was set at — and
/// dropping `offset_y` would flatten an accent that a face places above its
/// base. Both mistakes are invisible on a face with no mark positioning (every
/// bundled-face glyph has a zero offset, so the laws that read it are exact),
/// which is precisely why the arithmetic lives in one named function and is
/// tested directly.
fn on_path_pen(
    path: (f64, f64),
    up: (f64, f64),
    angle: f64,
    drop: f64,
    offset: (f64, f64),
) -> (f64, f64) {
    // Font y is up, the document's is down; the straight layout applies the
    // same flip (`Affine::straight` scales y by −1).
    let (dx, dy) = (offset.0, -offset.1);
    let (sin, cos) = angle.sin_cos();
    let (x, y) = path;
    (
        x - up.0 * drop + dx * cos - dy * sin,
        y - up.1 * drop + dx * sin + dy * cos,
    )
}

/// Lay a run along a path (RULE 2).
///
/// Each glyph is placed at its own **arc-length** position on the path, rotated
/// onto the tangent there, so the run genuinely follows the geometry rather
/// than approximating it with a straight baseline. `offset` slides the run
/// along the path in document units; alignment decides where the run sits
/// relative to that point (the same three cases as a straight run, measured
/// along the curve).
///
/// # The readability pass
///
/// A tangent alone is not enough. Half of a circle's tangents point backwards,
/// and text rotated onto one of those is upside down — legible, but not
/// readable. A glyph whose *up* would point into the lower half-plane is
/// therefore turned a further half turn, which is exactly the rule a designer
/// applies by hand when they set type on the bottom of a circle. The turn is
/// recorded per glyph ([`EvaluatedGlyph::angle`]), and the Evaluated Run Law
/// asserts it: no glyph of a bound run ever has its up pointing down.
///
/// Multi-line runs follow the path too: line `i` rides a parallel curve one
/// line advance toward that glyph's own down direction, so a paragraph on a
/// curve stays a paragraph.
pub fn layout_text_on_path(
    face: FaceRef<'_>,
    spec: &TextSpec<'_>,
    path: &Path,
    offset: f64,
) -> Result<EvaluatedText, TextError> {
    // The expensive half, from the cache: the `offset` is **not** part of the
    // key, so a slider drag, a variable bound to `path_offset` or a moving
    // circle all re-place this run instead of re-shaping it.
    let run = shaped_run(face, spec)?;
    let samples = sample_path(path);
    if samples.len() < 2 {
        // Nothing to follow — an empty sketch, or a single anchor. An empty run,
        // not an error: a normal state while the path is being drawn.
        return Ok(empty_run_from(&run.metrics));
    }
    let total = path_length(&samples);

    let mut glyphs: Vec<EvaluatedGlyph> = Vec::new();
    let mut outline = Path::builder();
    let mut truncated = 0usize;
    for local in &run.glyphs {
        // Where this glyph sits along the path: its own arc position in the run,
        // slid by the run's offset. `distance` is run-local, so the offset is the
        // only thing that moves here.
        let arc = offset + local.distance;
        // **Truncation** (RULE 2, §2.5): a run longer than its path — or slid
        // past either end — keeps the glyphs that fit and drops the rest, one at
        // a time, at *its own* arc position. The run is shorter, the survivors
        // keep their spacing, and nothing piles up at the seam: a hundred glyphs
        // clamped to the path's end would be a blot, not a word.
        if arc < 0.0 || arc > total {
            truncated += 1;
            continue;
        }
        let Some((path_x, path_y, tangent)) = sample_at(&samples, arc) else {
            continue;
        };
        // The readability pass: a glyph whose up would point down on screen
        // is turned a further half turn, which also — correctly — seats it
        // on the inside of the curve.
        let mut angle = tangent;
        if glyph_up(angle).1 > 0.0 {
            angle += PI;
        }
        let up = glyph_up(angle);
        // Later lines ride parallel curves, one line advance toward the
        // glyph's own down direction (the opposite of its up), carried by the
        // cached `line` index rather than by re-shaping per line.
        let drop = local.line as f64 * run.metrics.line_advance;
        let (pen_x, pen_y) = on_path_pen(
            (path_x, path_y),
            up,
            angle,
            drop,
            (local.offset_x, local.offset_y),
        );
        // **The placement**: rotate the cached outline about its own pen origin
        // and put that origin on the path —
        // `T(pen) ∘ R(θ) ∘ T(−local_pen)`, which is exactly the affine the
        // direct construction used to apply while it was still shaping. An
        // `offset` frame is this line and the event walk inside it; nothing
        // above re-reads the font.
        let local_pen = (local.distance + local.offset_x, drop - local.offset_y);
        let place = Affine::translate(-local_pen.0, -local_pen.1)
            .then(&Affine::rotate(angle))
            .then(&Affine::translate(pen_x, pen_y));
        let glyph = transformed_path(&local.outline, &place);
        append_path(&mut outline, &glyph);
        glyphs.push(EvaluatedGlyph {
            glyph_id: local.glyph_id,
            cluster: local.cluster,
            advance: local.advance,
            distance: arc,
            angle,
            outline: glyph,
        });
    }

    Ok(EvaluatedText {
        glyphs,
        outline: outline.build(),
        metrics: TextMetrics {
            // A bound run's own box is the arc-length window it covers, one line
            // advance tall per line: the number a UI needs to slide it, not a
            // bounding box that changes shape with the curve.
            width: total,
            height: run.metrics.lines as f64 * run.metrics.line_advance,
            ascender: run.metrics.ascender,
            descender: run.metrics.descender,
            line_advance: run.metrics.line_advance,
            lines: run.metrics.lines,
        },
        truncated,
    })
}

// ── outline to paths (RULE 3) ───────────────────────────────────────────────

/// One letterform of a run, ready to become a [`vectra_core::NodeKind::Path`].
///
/// `name` is the letterform's **source character**, so the layers panel reads
/// `V`, `e`, `c` after an outline instead of a row of uuids. `start` and
/// `segments` are ordinary path slots: the outlined letter is a first-class
/// path node in every sense — addressable by the solver, the AI planner and the
/// pen like any other.
#[derive(Debug, Clone, PartialEq)]
pub struct OutlinePlan {
    pub name: String,
    pub start: Point2,
    pub segments: Vec<PathSegment>,
}

/// The source character a glyph was shaped from, for naming its outline.
fn glyph_character(text: &str, cluster: usize) -> String {
    text.get(cluster..)
        .and_then(|rest| rest.chars().next())
        .map(|character| character.to_string())
        .unwrap_or_else(|| "•".to_string())
}

/// **Turn a laid-out run into path plans** (RULE 3): one closed plan per glyph,
/// contours and counters included.
///
/// # How a glyph with a counter becomes *one* path
///
/// The path model is a start point plus segments — there is no `MoveTo` — so a
/// glyph cannot be two disjoint subpaths (`o` is a ring inside a ring). The
/// encoding used here is the classic **keyhole**: the outer contour is closed,
/// then a zero-width bridge runs out to the counter, the counter is traced, and
/// the bridge returns. The bridge is traversed once in each direction, so it
/// contributes no area and cancels under **both** fill rules the workspace
/// uses: the renderer fills even-odd, and TrueType's nonzero windings are
/// opposite for a counter — either way the hole is a hole and the letter looks
/// exactly like the type it replaces.
///
/// The alternative — one node per contour — would draw the counters solid,
/// because a fill rule applies per path and cannot see a sibling node.
pub fn outline_plans(run: &EvaluatedText, text: &str) -> Vec<OutlinePlan> {
    let mut plans = Vec::new();
    for glyph in &run.glyphs {
        let rings = path_to_rings(&glyph.outline, OUTLINE_TOLERANCE);
        // A contour needs three points before it encloses anything.
        let rings: Vec<&Vec<Point2>> = rings.iter().filter(|ring| ring.len() >= 3).collect();
        let Some(outer) = rings.first() else {
            continue;
        };
        let start = outer[0];
        let mut segments: Vec<PathSegment> = Vec::new();
        for point in &outer[1..] {
            segments.push(line(*point));
        }
        segments.push(PathSegment::Close);
        for ring in &rings[1..] {
            segments.push(line(ring[0]));
            for point in &ring[1..] {
                segments.push(line(*point));
            }
            segments.push(line(ring[0]));
            segments.push(line(start));
            segments.push(PathSegment::Close);
        }
        plans.push(OutlinePlan {
            name: glyph_character(text, glyph.cluster),
            start,
            segments,
        });
    }
    plans
}

fn line(point: Point2) -> PathSegment {
    PathSegment::Line {
        to: Parameter::Literal(point),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::ResolvedSegment;
    use crate::scene::EvaluatedPrimitive;

    fn spec(text: &str) -> TextSpec<'_> {
        TextSpec::new(text, BUNDLED_FAMILY, 100.0)
    }

    fn circle(radius: f64) -> Path {
        crate::paths::primitive_to_curve_path(&EvaluatedPrimitive::Circle {
            cx: 0.0,
            cy: 0.0,
            r: radius,
        })
    }

    #[test]
    fn bundled_face_parses_and_covers_the_latin_range() {
        let face = ttf_parser::Face::parse(BUNDLED_FACE, 0).expect("bundled face parses");
        assert_eq!(face.units_per_em(), 2048);
        for character in ['A', 'z', '1', '&', '—', '™'] {
            assert!(
                face.glyph_index(character).is_some(),
                "{character} must be in the bundled face"
            );
        }
    }

    #[test]
    fn a_run_outlines_one_glyph_per_character() {
        let run = layout_text(FaceRef::bundled(), &spec("Vectra"), (0.0, 0.0)).expect("layout");
        assert_eq!(run.glyphs.len(), 6);
        assert!(run.metrics.width > 0.0);
        assert!(run.metrics.ascender > 0.0 && run.metrics.descender < 0.0);
        // The merged outline is non-empty and its box straddles the baseline:
        // the caps sit above it (negative y in the document's y-down space) and
        // the merged box's bottom edge is the baseline itself, because every
        // glyph of "Vectra" sits on it.
        assert!(!run.glyphs.is_empty(), "an empty run draws nothing");
        let (min_x, min_y, max_x, max_y) = crate::paths::path_bounds(&run.outline);
        assert!(min_x < max_x && min_y < max_y, "the run has a box");
        assert!(min_y < 0.0, "capitals reach above the baseline");
        assert!(
            (max_y - 0.0).abs() < 2.0,
            "no descenders: the box's bottom is the baseline ({max_y})"
        );
    }

    #[test]
    fn font_size_scales_the_run() {
        let big = layout_text(FaceRef::bundled(), &spec("Aa"), (0.0, 0.0)).unwrap();
        let mut small_spec = spec("Aa");
        small_spec.size = 50.0;
        let small = layout_text(FaceRef::bundled(), &small_spec, (0.0, 0.0)).unwrap();
        let ratio = big.metrics.width / small.metrics.width;
        assert!(
            (ratio - 2.0).abs() < 1e-9,
            "width scales with size: {ratio}"
        );
        // …and so does the box the renderer, the hit test and the selection
        // rectangle all read: halve the size and the outline's height halves.
        let (_, big_top, _, big_bottom) = crate::paths::path_bounds(&big.outline);
        let (_, small_top, _, small_bottom) = crate::paths::path_bounds(&small.outline);
        let big_height = big_bottom - big_top;
        let small_height = small_bottom - small_top;
        assert!(
            (big_height / small_height - 2.0).abs() < 1e-6,
            "the drawn box scales with the parameter: {big_height} vs {small_height}"
        );
    }

    #[test]
    fn alignment_anchors_the_run_on_the_origin() {
        let width = layout_text(FaceRef::bundled(), &spec("Vectra"), (0.0, 0.0))
            .unwrap()
            .metrics
            .width;
        for (alignment, expected) in [
            (TextAlign::Left, 0.0),
            (TextAlign::Center, -width * 0.5),
            (TextAlign::Right, -width),
        ] {
            let mut request = spec("Vectra");
            request.alignment = alignment;
            let run = layout_text(FaceRef::bundled(), &request, (10.0, 20.0)).unwrap();
            let first = run.glyphs.first().expect("a glyph");
            let (min_x, _, _, _) = crate::paths::path_bounds(&first.outline);
            let pen = 10.0 + expected;
            assert!(
                (min_x - pen).abs() < 2.0,
                "{alignment:?} puts the first glyph at {min_x} (the pen is at {pen})"
            );
        }
    }

    #[test]
    fn letter_spacing_widens_the_run() {
        let mut tracked = spec("Vectra");
        tracked.letter_spacing = 4.0;
        let plain = layout_text(FaceRef::bundled(), &spec("Vectra"), (0.0, 0.0)).unwrap();
        let spaced = layout_text(FaceRef::bundled(), &tracked, (0.0, 0.0)).unwrap();
        assert!(spaced.metrics.width > plain.metrics.width);
        assert!((spaced.metrics.width - plain.metrics.width - 24.0).abs() < 1e-9);
    }

    #[test]
    fn line_height_sets_the_baseline_distance() {
        let mut two = spec("ab\nab");
        two.line_height = 2.0;
        let run = layout_text(FaceRef::bundled(), &two, (0.0, 0.0)).unwrap();
        assert_eq!(run.metrics.lines, 2);
        let (_, first_top, _, _) = crate::paths::path_bounds(&run.glyphs[0].outline);
        let (_, second_top, _, _) = crate::paths::path_bounds(&run.glyphs[2].outline);
        assert!(
            (second_top - first_top - 200.0).abs() < 0.5,
            "the second line starts one line-advance down: {}",
            second_top - first_top
        );
    }

    #[test]
    fn empty_text_is_a_total_empty_run() {
        let run = layout_text(FaceRef::bundled(), &spec(""), (0.0, 0.0)).expect("empty layout");
        assert!(run.glyphs.is_empty());
        assert!(run.glyphs.is_empty(), "a missing face must draw nothing");
        assert_eq!(run.metrics.width, 0.0);
    }

    #[test]
    fn a_bound_run_rotates_onto_the_tangent_and_stays_upright() {
        let path = circle(200.0);
        let samples = sample_path(&path);
        let run =
            layout_text_on_path(FaceRef::bundled(), &spec("readable"), &path, 0.0).expect("layout");
        assert_eq!(run.glyphs.len(), 8);
        for glyph in &run.glyphs {
            // The readability pass, as an assertion: a glyph's up direction
            // never points down on screen…
            assert!(
                glyph_up(glyph.angle).1 <= 0.0,
                "glyph {} is upside down at angle {}",
                glyph.glyph_id,
                glyph.angle
            );
            // …and it *is* the tangent of the path at its own arc position,
            // modulo the half turn the pass may have applied.
            let (_, _, tangent) = sample_at(&samples, glyph.distance).unwrap();
            let difference = (glyph.angle - tangent).abs() % PI;
            assert!(
                difference < 1e-9 || (PI - difference).abs() < 1e-9,
                "glyph {} sits at {tangent} but is rotated {}",
                glyph.glyph_id,
                glyph.angle
            );
        }
    }

    #[test]
    fn the_offset_slides_a_bound_run_along_the_path() {
        let path = circle(100.0);
        let first = layout_text_on_path(FaceRef::bundled(), &spec("slide"), &path, 0.0).unwrap();
        let slid = layout_text_on_path(FaceRef::bundled(), &spec("slide"), &path, 40.0).unwrap();
        let moved = slid.glyphs[0].distance - first.glyphs[0].distance;
        assert!((moved - 40.0).abs() < 1e-9, "the run slid {moved} units");
        let a = crate::paths::path_bounds(&first.glyphs[0].outline);
        let b = crate::paths::path_bounds(&slid.glyphs[0].outline);
        let travelled = ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
        assert!(
            travelled > 10.0,
            "the glyph moved {travelled} units on screen"
        );
    }

    #[test]
    fn a_run_on_a_degenerate_path_is_empty_rather_than_broken() {
        let empty = Path::builder().build();
        let run = layout_text_on_path(FaceRef::bundled(), &spec("x"), &empty, 0.0).unwrap();
        assert!(run.glyphs.is_empty());
        assert_eq!(run.metrics.lines, 1);
    }

    #[test]
    fn outlining_yields_one_closed_plan_per_letterform() {
        let run = layout_text(FaceRef::bundled(), &spec("lo"), (40.0, 80.0)).unwrap();
        let plans = outline_plans(&run, "lo");
        assert_eq!(plans.len(), 2, "one plan per shaped glyph");
        assert_eq!(plans[0].name, "l");
        assert_eq!(plans[1].name, "o");
        for plan in &plans {
            assert!(plan.segments.len() >= 4, "a letterform has contours");
            assert!(
                matches!(plan.segments.last(), Some(PathSegment::Close)),
                "every planned letterform ends closed"
            );
            for segment in &plan.segments {
                let PathSegment::Line { to } = segment else {
                    continue;
                };
                let Parameter::Literal(point) = to else {
                    panic!("an outline's points are literals");
                };
                assert!(point.x.is_finite() && point.y.is_finite());
            }
        }
        // The counter travels inside the 'o' plan: its keyhole is measurably
        // bigger than the straight-sided 'l'.
        assert!(plans[1].segments.len() > plans[0].segments.len());
    }

    #[test]
    fn an_outlined_letterform_rebuilds_into_the_same_picture() {
        let run = layout_text(FaceRef::bundled(), &spec("o"), (0.0, 0.0)).unwrap();
        let plans = outline_plans(&run, "o");
        let plan = plans.first().expect("one plan");
        let mut resolved = Vec::new();
        for segment in &plan.segments {
            match segment {
                PathSegment::Line { to } => {
                    let Parameter::Literal(point) = to else {
                        panic!("literal");
                    };
                    resolved.push(ResolvedSegment::Line { to: *point });
                }
                PathSegment::Close => resolved.push(ResolvedSegment::Close),
                other => panic!("an outline is lines and closes only, got {other:?}"),
            }
        }
        let rebuilt = crate::paths::build_path(plan.start, &resolved);
        let original = crate::paths::path_bounds(&run.glyphs[0].outline);
        let after = crate::paths::path_bounds(&rebuilt);
        for (label, a, b) in [
            ("min x", original.0, after.0),
            ("min y", original.1, after.1),
            ("max x", original.2, after.2),
            ("max y", original.3, after.3),
        ] {
            assert!(
                (a - b).abs() < 0.2,
                "{label} drifts rebuilding the outline: {a} vs {b}"
            );
        }
    }

    #[test]
    fn the_font_library_refuses_bytes_that_are_not_a_font() {
        let mut library = FontLibrary::new();
        assert!(library.register("Broken", vec![0u8; 32]).is_err());
        assert!(library
            .register("Vectra Sans", BUNDLED_FACE.to_vec())
            .is_ok());
        assert_eq!(
            library.face_bytes("vectra  SANS").map(<[u8]>::len),
            Some(BUNDLED_FACE.len())
        );
        // The **spelled** name comes back, not the internal key: a picker shows
        // the typeface a designer registered, not the string the lookup uses.
        assert_eq!(library.families(), vec!["Vectra Sans".to_string()]);
    }

    #[test]
    fn the_bundled_library_names_one_family_however_many_aliases_it_has() {
        let library = FontLibrary::bundled();
        // `sans-serif` and `system-ui` resolve…
        for alias in BUNDLED_ALIASES {
            assert!(
                library.face_bytes(alias).is_some(),
                "{alias} must resolve to the bundled face"
            );
        }
        // …but they are *spellings* of that face, not extra typefaces: a picker
        // offering four names for one font would be lying about the library.
        assert_eq!(library.families(), vec![BUNDLED_FAMILY.to_string()]);
    }

    /// A glyph's own attachment offset (`offset_x`/`offset_y`, font-space signs)
    /// is a vector in the glyph's **unrotated** frame: it rotates with the run.
    ///
    /// The bundled face has no mark positioning, so every glyph it shapes has a
    /// zero offset — which is exactly why this is tested on the arithmetic
    /// itself: the mistake (sliding the offset along the tangent, or dropping
    /// `offset_y`) is invisible on every asset in this repository and would
    /// only surface when a host registers a face with marks in it.
    #[test]
    fn a_glyphs_own_offset_rotates_with_the_run() {
        // 6 units along the baseline, 4 units *up* — the shaper's signs.
        let offset = (6.0, 4.0);
        // On a horizontal tangent the pen is the straight pen: +6 along x, and
        // the 4 upward units land above the baseline in a y-down document.
        let (x, y) = on_path_pen((0.0, 0.0), glyph_up(0.0), 0.0, 0.0, offset);
        assert!((x - 6.0).abs() < 1e-12, "{x}");
        assert!((y + 4.0).abs() < 1e-12, "{y}");
        // A quarter turn: the vector turns with the glyph — R(π/2)·(6, −4).
        let angle = PI / 2.0;
        let (x, y) = on_path_pen((0.0, 0.0), glyph_up(angle), angle, 0.0, offset);
        assert!((x - 4.0).abs() < 1e-12, "{x}");
        assert!((y - 6.0).abs() < 1e-12, "{y}");
        // Neither component survives unchanged, so neither "slide along the
        // tangent" nor "ignore the y offset" passes this.
        assert!((x - 6.0).abs() > 1.0 && (y + 4.0).abs() > 1.0);
    }

    /// Later lines drop toward the glyph's own **down**, which is the opposite
    /// of its up — and the drop is perpendicular to the tangent rather than
    /// along it, so a paragraph on a curve stays a paragraph.
    #[test]
    fn a_later_line_drops_toward_the_glyphs_own_down() {
        let (x, y) = on_path_pen((0.0, 0.0), glyph_up(0.0), 0.0, 10.0, (0.0, 0.0));
        assert!((x).abs() < 1e-12 && (y - 10.0).abs() < 1e-12, "({x}, {y})");
        // A reachable rotated case (the readability pass leaves up.y ≤ 0):
        // the displacement is exactly `-drop × up`.
        let angle = PI / 4.0;
        let up = glyph_up(angle);
        let (x, y) = on_path_pen((0.0, 0.0), up, angle, 10.0, (0.0, 0.0));
        assert!((x + up.0 * 10.0).abs() < 1e-12, "{x}");
        assert!((y + up.1 * 10.0).abs() < 1e-12, "{y}");
        assert!(y > 0.0, "a second line sits below the first on screen: {y}");
    }

    #[test]
    fn an_unknown_family_falls_back_to_the_bundled_face_and_says_so() {
        let (bytes, substituted) = resolve_face(None, "Helvetica Neu");
        assert_eq!(bytes.bytes.len(), BUNDLED_FACE.len());
        assert!(substituted);
        let (_, known) = resolve_face(None, BUNDLED_FAMILY);
        assert!(!known);
    }

    #[test]
    fn a_library_face_is_used_instead_of_the_bundled_one() {
        let mut library = FontLibrary::new();
        library
            .register("Vectra Sans", BUNDLED_FACE.to_vec())
            .unwrap();
        let (bytes, substituted) = resolve_face(Some(&library), "Vectra Sans");
        // The *bytes* are the registered ones — the test's point is the branch,
        // which a registered family takes without any fallback flag.
        assert_eq!(bytes.bytes.len(), BUNDLED_FACE.len());
        assert!(!substituted);
        assert!(library.face_bytes("vectra sans").is_some());
    }
    // ── the shaping cache ───────────────────────────────────────────────────

    /// **The offset is a placement, not a re-shape** (Task 11.0 performance).
    ///
    /// The claim is about *work done*, so the test reads the cache counters:
    /// two layouts whose only difference is the offset must shape once, and a
    /// font-size change must shape again. The geometry assertion is there so the
    /// test cannot pass by returning something stale: the second run's glyphs
    /// are drawn in different places.
    #[test]
    fn an_offset_change_re_places_instead_of_re_shaping() {
        let spec = TextSpec::new("Vectra", BUNDLED_FAMILY, 40.0);
        let path = circle(300.0);
        clear_shape_cache();

        let still = layout_text_on_path(FaceRef::bundled(), &spec, &path, 0.0).unwrap();
        assert_eq!(still.truncated, 0, "the run fits at offset 0");
        let after_first = shape_cache_stats();
        assert_eq!(after_first.misses, 1, "one distinct run, one shaping");

        let slid = layout_text_on_path(FaceRef::bundled(), &spec, &path, 120.0).unwrap();
        let after_slide = shape_cache_stats();
        assert_eq!(
            after_slide.misses, 1,
            "sliding must not reshape: {after_slide:?}"
        );
        assert_eq!(after_slide.hits, 1, "{after_slide:?}");
        assert_eq!(slid.glyphs.len(), still.glyphs.len());
        // …and it really moved: the same glyphs, drawn in other places.
        let first_box = |run: &EvaluatedText| crate::paths::path_bounds(&run.glyphs[0].outline);
        let (a, b) = (first_box(&still), first_box(&slid));
        let moved = ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
        assert!(moved > 10.0, "the run did not move: {moved}");

        // The typography *is* the key: a size change shapes a new run.
        let bigger = TextSpec { size: 64.0, ..spec };
        let _ = layout_text_on_path(FaceRef::bundled(), &bigger, &path, 0.0).unwrap();
        let after_size = shape_cache_stats();
        assert_eq!(
            after_size.misses, 2,
            "a font-size change is a new shaped run: {after_size:?}"
        );
    }

    /// **A cache hit is indistinguishable from a cold layout.**
    ///
    /// The equivalence is exact — glyph for glyph, contour for contour — because
    /// a hit that differed from a miss by a hair would be a per-frame wobble the
    /// moment the cache warmed or evicted.
    #[test]
    fn a_cached_re_placement_equals_a_cold_layout() {
        let spec = TextSpec::new("AaVv", BUNDLED_FAMILY, 33.0);
        let path = circle(180.0);

        // Warm: shape once, then re-place by sliding.
        clear_shape_cache();
        let first = layout_text_on_path(FaceRef::bundled(), &spec, &path, 0.0).unwrap();
        assert_eq!(first.truncated, 0);
        let warm = layout_text_on_path(FaceRef::bundled(), &spec, &path, 77.0).unwrap();
        assert_eq!(shape_cache_stats().misses, 1, "the slide must be a hit");

        // Cold: the same placement from an empty cache.
        clear_shape_cache();
        let cold = layout_text_on_path(FaceRef::bundled(), &spec, &path, 77.0).unwrap();
        assert_eq!(shape_cache_stats().misses, 1);

        assert_eq!(warm, cold, "a cache hit must equal a cold layout exactly");
    }

    /// The face a memo remembers is the **identity of the bytes**, not the
    /// family name: a host that re-registers a modified face under a name it
    /// used before must not be served the old outlines.
    #[test]
    fn a_re_registered_face_is_a_different_face() {
        let mut library = FontLibrary::new();
        library
            .register("House Face", BUNDLED_FACE.to_vec())
            .unwrap();
        let before = library.face_id("house face").expect("registered");
        assert_eq!(before, BUNDLED_FACE_ID, "the bundled bytes are themselves");
        assert_eq!(
            library.face_id("House  FACE"),
            Some(before),
            "identity survives how the family is spelled"
        );

        // The same family, re-registered from a byte-modified copy: the id must
        // change, because the memo keys on it.
        let mut altered = BUNDLED_FACE.to_vec();
        altered.extend_from_slice(&[0, 0, 0, 0]);
        library.register("house face", altered).unwrap();
        let after = library.face_id("house face").expect("still registered");
        assert_ne!(
            before, after,
            "different bytes must have different identities"
        );

        // And the memo honours it: same family, two identities, two shapers.
        let spec = TextSpec::new("Vectra", "House Face", 40.0);
        clear_shape_cache();
        let first = layout_text(FaceRef::new(BUNDLED_FACE, before), &spec, (0.0, 0.0)).unwrap();
        let again = layout_text(FaceRef::new(BUNDLED_FACE, after), &spec, (0.0, 0.0)).unwrap();
        assert_eq!(
            shape_cache_stats().misses,
            2,
            "identity, not family, is the key"
        );
        assert_eq!(first, again, "the same bytes draw the same picture");
    }

    /// Truncation: a run longer than its path keeps the glyphs that fit, each at
    /// its own arc position — the survivors keep their spacing and nothing piles
    /// up at the seam.
    #[test]
    fn an_overlong_run_keeps_the_window_that_fits() {
        let line_of = |length: f64| {
            let mut builder = lyon::path::Builder::new();
            builder.begin(lyon::math::point(0.0, 0.0));
            builder.line_to(lyon::math::point(length as f32, 0.0));
            builder.end(false);
            builder.build()
        };
        let spec = TextSpec::new("mmmmmmmm", BUNDLED_FAMILY, 100.0);
        // A straight 200-unit line, and a run of eight `m`s that cannot fit it.
        let line = line_of(200.0);
        // The same run on a line long enough that **nothing** is left out: the
        // unclipped reference the window is defined against.
        let reference =
            layout_text_on_path(FaceRef::bundled(), &spec, &line_of(2_000.0), 0.0).unwrap();
        assert_eq!(reference.truncated, 0, "the reference must be unclipped");
        assert_eq!(reference.glyphs.len(), 8);
        let run = layout_text_on_path(FaceRef::bundled(), &spec, &line, 0.0).unwrap();
        assert!(run.truncated > 0, "the run must not fit");
        assert_eq!(run.glyphs.len() + run.truncated, 8);
        // Every drawn glyph is on the path, in order, spaced by its own advance.
        let mut previous: Option<(f64, f64)> = None;
        for glyph in &run.glyphs {
            assert!(
                (0.0..=200.0).contains(&glyph.distance),
                "a drawn glyph is off the path: {}",
                glyph.distance
            );
            if let Some((distance, advance)) = previous {
                assert!(
                    (glyph.distance - (distance + advance)).abs() < 1e-9,
                    "the survivors must keep their spacing"
                );
            }
            previous = Some((glyph.distance, glyph.advance));
        }
        // Sliding keeps *exactly* the unclipped run's glyphs whose arc position
        // is still on the path, each where the slide put it: the dropped glyphs
        // are the ones off either end, and no survivor moves relative to its
        // neighbours. This is the strongest form of the truncation claim — it
        // fails if a glyph piles up at the seam, or if the window is chosen by
        // anything other than the glyph's own arc position.
        let window = |offset: f64| -> Vec<(u16, f64)> {
            reference
                .glyphs
                .iter()
                .map(|glyph| (glyph.glyph_id, glyph.distance + offset))
                .filter(|(_, distance)| (0.0..=200.0).contains(distance))
                .collect()
        };
        for offset in [-60.0, 0.0, 60.0] {
            let slid = layout_text_on_path(FaceRef::bundled(), &spec, &line, offset).unwrap();
            let actual: Vec<(u16, f64)> = slid
                .glyphs
                .iter()
                .map(|glyph| (glyph.glyph_id, glyph.distance))
                .collect();
            assert_eq!(actual, window(offset), "at offset {offset}");
            assert_eq!(
                slid.truncated,
                8 - slid.glyphs.len(),
                "every glyph left out is counted, at offset {offset}"
            );
        }
        // A slide past the end keeps nothing — and still reports every glyph it
        // left out, so the evaluator can say how much of the run is off-path.
        let past = layout_text_on_path(FaceRef::bundled(), &spec, &line, 1_000.0).unwrap();
        assert!(past.glyphs.is_empty());
        assert_eq!(past.truncated, 8);
    }
}
