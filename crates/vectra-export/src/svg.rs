//! The **semantic SVG exporter** (Task 8.0, RULE 1).
//!
//! The rule: "The SVG exporter must preserve mathematical primitives. A `Circle`
//! must export as `<circle cx="…" cy="…" r="…" />`, not a `<path>` with bezier
//! approximations. An `Arc` must use the SVG `A` command. Only true `Path` nodes
//! export as `<path>`."
//!
//! So this module is a *tag* decision table, not a tessellator:
//!
//! | IR | SVG |
//! | --- | --- |
//! | [`ExportGeometry::Rect`] | `<rect x y width height/>` (+ `rx` when rounded) |
//! | [`ExportGeometry::Circle`] | `<circle cx cy r/>` |
//! | [`ExportGeometry::Arc`] | `<path d="M… A…"/>` — one `A`, never a bezier chain |
//! | [`ExportGeometry::Path`] | `<path d="…"/>`, the lyon serialization the IR carries |
//! | [`ExportGeometry::Group`] | `<g>` (children draw as their own nodes) |
//!
//! ## Coordinates: the document is y-up, SVG is y-down
//!
//! The document's y axis points up (that is what the renderer's camera and the
//! inspector both assume), while SVG's points down. The exporter therefore emits
//! geometry in **document coordinates** and mirrors the whole drawing once:
//!
//! ```text
//! <g transform="translate(0 {maxY}) scale(1 -1)">
//! ```
//!
//! which is exactly `y_svg = maxY - y_doc`. Keeping the numbers in document space
//! is what makes an exported picture readable next to the IR (and next to the
//! inspector): every `x`/`width` is the number the user typed.
//!
//! The mirror also settles the arc flags: inside that group the local coordinate
//! system is the document's, so the `A` command's sweep flag is simply "the
//! canonical arc increases its angle", and the flip is applied to the finished
//! path — the flags never have to be reasoned about in screen space.

use crate::ir::{
    fmt_number, ExportBounds, ExportColor, ExportGeometry, ExportIR, ExportNode, ExportParam,
};

/// Knobs a caller may want; the defaults are what the UI uses.
#[derive(Debug, Clone)]
pub struct SvgOptions {
    /// Write `data-vectra-node="…"` on every element, so an importer can line the
    /// picture back up with the document.
    pub include_node_ids: bool,
    /// Padding around the drawing, as a fraction of its larger dimension (never
    /// below 1 unit) — a stroke needs somewhere to land.
    pub pad: f64,
    /// An **explicit frame** in document space, overriding the drawing's own
    /// bounds. This is what "Export current artboard" means: the picture is
    /// cropped to the board the designer was working on, not to the ink (Task
    /// 10.2 RULE 2). `None` frames the ink, as every export did before.
    pub frame: Option<ExportBounds>,
    /// A background painted under the artwork across the frame — an artboard's
    /// own colour. `None` keeps the document transparent.
    pub background: Option<ExportColor>,
}

impl Default for SvgOptions {
    fn default() -> Self {
        Self {
            include_node_ids: true,
            pad: 0.02,
            frame: None,
            background: None,
        }
    }
}

/// Export the IR as an SVG document.
pub fn export_svg(ir: &ExportIR) -> String {
    export_svg_with(ir, &SvgOptions::default())
}

/// Export the IR as an SVG document, with options.
pub fn export_svg_with(ir: &ExportIR, options: &SvgOptions) -> String {
    let bounds = options.frame.unwrap_or_else(|| frame(ir, options.pad));
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"{minx} 0 {w} {h}\" fill=\"none\">\n",
        w = fmt_number(bounds.width()),
        h = fmt_number(bounds.height()),
        minx = fmt_number(bounds.min_x),
    ));
    out.push_str(&format!(
        "  <!-- Vectra export: {} node(s), {} prop(s) -->\n",
        ir.nodes.len(),
        ir.props.len()
    ));
    out.push_str(&format!(
        "  <g transform=\"translate(0 {}) scale(1 -1)\">\n",
        fmt_number(bounds.max_y)
    ));
    if let Some(background) = &options.background {
        // Inside the mirroring group, so the rect is stated in document space
        // like every other number the exporter writes.
        out.push_str(&format!(
            "    <rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\"/>\n",
            fmt_number(bounds.min_x),
            fmt_number(bounds.min_y),
            fmt_number(bounds.width()),
            fmt_number(bounds.height()),
            escape_xml(&background.code)
        ));
    }
    for node in &ir.nodes {
        let element = element(node, options);
        if !element.is_empty() {
            out.push_str("    ");
            out.push_str(&element);
            out.push('\n');
        }
    }
    out.push_str("  </g>\n</svg>\n");
    out
}

/// The viewport, in SVG space: the document's box, padded, with `min_y` pinned
/// to zero (the mirror maps `max_y` to the top).
fn frame(ir: &ExportIR, pad: f64) -> ExportBounds {
    let Some(bounds) = ir.bounds else {
        return ExportBounds {
            min_x: 0.0,
            min_y: 0.0,
            max_x: 0.0,
            max_y: 0.0,
        };
    };
    let margin = (bounds.width().max(bounds.height()) * pad).max(1.0);
    ExportBounds {
        min_x: bounds.min_x - margin,
        min_y: 0.0,
        max_x: bounds.max_x + margin,
        max_y: bounds.max_y + margin,
    }
}

/// One artboard, as the exporter needs to see it: a frame, a background, and
/// the artwork that belongs to it (Task 10.2 RULE 2).
///
/// The *document* knows which layer a node is on; the exporter only needs the
/// answer, so the caller passes it in. That keeps this crate free of the layer
/// registry while still being the one place that knows how a board is drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct ArtboardView {
    pub id: String,
    pub name: String,
    pub bounds: ExportBounds,
    pub background: ExportColor,
    /// The node ids the board draws, in draw order.
    pub nodes: Vec<String>,
}

/// Which boards an export covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtboardScope {
    /// Only the active board: the artwork is cropped to its frame.
    Current,
    /// Every board, laid out in one document at its own place in document
    /// space — which is exactly how the canvas shows them, so the exported file
    /// and the screen agree.
    All,
}

/// Export `ir` scoped to one or more artboards (Task 10.2 RULE 2).
///
/// Each board contributes a `<g>` with its background rect, its nodes filtered
/// to the ones it lists, and a `data-vectra-artboard` attribute naming it. The
/// frame is the union of the boards in scope — so "export all" produces a
/// contact sheet of the document's canvases, and "export current" produces
/// exactly one.
pub fn export_artboards_svg(
    ir: &ExportIR,
    boards: &[ArtboardView],
    scope: ArtboardScope,
) -> String {
    let selected: Vec<&ArtboardView> = match scope {
        ArtboardScope::Current => boards.iter().take(1).collect(),
        ArtboardScope::All => boards.iter().collect(),
    };
    let frame = selected
        .iter()
        .map(|board| board.bounds)
        .reduce(ExportBounds::union)
        .unwrap_or_else(|| frame(ir, 0.0));

    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"{minx} 0 {w} {h}\" fill=\"none\">\n",
        w = fmt_number(frame.width().max(1.0)),
        h = fmt_number(frame.height().max(1.0)),
        minx = fmt_number(frame.min_x),
    ));
    out.push_str(&format!(
        "  <!-- Vectra artboard export: {} board(s), {} node(s) -->\n",
        selected.len(),
        selected
            .iter()
            .map(|board| board.nodes.len())
            .sum::<usize>()
    ));
    out.push_str(&format!(
        "  <g transform=\"translate(0 {}) scale(1 -1)\">\n",
        fmt_number(frame.max_y)
    ));
    let options = SvgOptions {
        include_node_ids: true,
        pad: 0.0,
        frame: None,
        background: None,
    };
    for board in selected {
        out.push_str(&format!(
            "    <g data-vectra-artboard=\"{}\" data-vectra-artboard-name=\"{}\">\n",
            escape_xml(&board.id),
            escape_xml(&board.name)
        ));
        out.push_str(&format!(
            "      <rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\"/>\n",
            fmt_number(board.bounds.min_x),
            fmt_number(board.bounds.min_y),
            fmt_number(board.bounds.width()),
            fmt_number(board.bounds.height()),
            escape_xml(&board.background.code)
        ));
        for node in &ir.nodes {
            if !board.nodes.contains(&node.id) {
                continue;
            }
            let element = element(node, &options);
            if !element.is_empty() {
                out.push_str("      ");
                out.push_str(&element);
            }
        }
        out.push_str("    </g>\n");
    }
    out.push_str("  </g>\n</svg>\n");
    out
}

/// One node, one element (or one element and a comment, or nothing).
fn element(node: &ExportNode, options: &SvgOptions) -> String {
    let id = if options.include_node_ids {
        format!(" data-vectra-node=\"{}\"", escape_xml(&node.id))
    } else {
        String::new()
    };
    let style = paint(
        &node.style.fill,
        &node.style.stroke,
        &node.style.stroke_width,
        &node.style.opacity,
    );
    match &node.geometry {
        ExportGeometry::Rect {
            x,
            y,
            width,
            height,
            corner_radius,
        } => {
            let rounded = if corner_radius.number() > 0.0 {
                format!(" rx=\"{}\"", number(corner_radius))
            } else {
                String::new()
            };
            format!(
                "<rect{id} x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"{rounded}{style}/>",
                number(x),
                number(y),
                number(width),
                number(height),
            )
        }
        ExportGeometry::Circle { cx, cy, radius } => format!(
            "<circle{id} cx=\"{}\" cy=\"{}\" r=\"{}\"{style}/>",
            number(cx),
            number(cy),
            fmt_number(radius.number().abs()),
        ),
        ExportGeometry::Arc {
            cx,
            cy,
            radius,
            start_angle,
            end_angle,
        } => {
            let d = arc_data(
                cx.number(),
                cy.number(),
                radius.number().abs(),
                start_angle.number(),
                end_angle.number(),
            );
            format!("<path{id} d=\"{d}\"{style}/>")
        }
        ExportGeometry::Path { d } => match d {
            Some(d) => format!("<path{id} d=\"{d}\"{style}/>"),
            // Nothing was resolved for this path (its points come from a clock or
            // a procedural read). A picture cannot invent them; say so in place.
            None => format!(
                "<!-- {id}: path `{}` has no resolved geometry -->",
                escape_xml(&node.name)
            ),
        },
        ExportGeometry::Group { children } => {
            format!("<g{id} data-vectra-children=\"{}\"/>", children.len())
        }
    }
}

/// A number the SVG can carry.
fn number(param: &ExportParam) -> String {
    fmt_number(param.number())
}

/// `fill`, `stroke`, `stroke-width`, `opacity` — emitted only when they change
/// the default, so the output reads like something a person would write.
fn paint(
    fill: &ExportColor,
    stroke: &ExportColor,
    stroke_width: &ExportParam,
    opacity: &ExportParam,
) -> String {
    let mut out = String::new();
    match fill.value {
        Some(color) if color.a == 0 => out.push_str(" fill=\"none\""),
        Some(color) if color.a == 255 => {
            if fill.is_parametric() {
                // A colour slot can only be parametric today through a variable,
                // which the IR has already resolved into `value`; the code path is
                // kept for the day a colour variable exists.
                out.push_str(&format!(" fill=\"{}\"", escape_xml(&fill.code)));
            } else {
                out.push_str(&format!(" fill=\"{}\"", color.to_hex()));
            }
        }
        Some(color) => out.push_str(&format!(
            " fill=\"{}\" fill-opacity=\"{}\"",
            rgb_hex(color),
            fmt_number(color.a as f64 / 255.0)
        )),
        None => out.push_str(" fill=\"none\""),
    }

    let width = stroke_width.number();
    let stroked = stroke.value.map(|color| color.a > 0).unwrap_or(false) && width > 0.0;
    if stroked {
        if stroke.is_parametric() {
            out.push_str(&format!(" stroke=\"{}\"", escape_xml(&stroke.code)));
        } else if let Some(color) = stroke.value {
            let hex = if color.a == 255 {
                color.to_hex()
            } else {
                rgb_hex(color)
            };
            out.push_str(&format!(" stroke=\"{hex}\""));
            if color.a != 255 {
                out.push_str(&format!(
                    " stroke-opacity=\"{}\"",
                    fmt_number(color.a as f64 / 255.0)
                ));
            }
        }
        out.push_str(&format!(" stroke-width=\"{}\"", fmt_number(width)));
    }

    let alpha = opacity.number();
    if (alpha - 1.0).abs() > 1e-9 {
        out.push_str(&format!(" opacity=\"{}\"", fmt_number(alpha)));
    }
    out
}

fn rgb_hex(color: vectra_core::geom::Color) -> String {
    let hex = color.to_hex();
    hex.chars().take(7).collect()
}

/// An arc as **one** SVG `A` command.
///
/// The evaluator canonicalises an arc to `(start, start + sweep)` with
/// `sweep ∈ [0, TAU]`, so:
///
/// * `large-arc-flag` is `sweep > π`;
/// * `sweep-flag` is `1` — the canonical arc always runs in the positive-angle
///   direction, and positive angle in the document's (y-up) space *is* sweep-flag
///   `1` in the mirrored group this exporter emits;
/// * a full sweep (`TAU`) is two half arcs, because a single `A` with coincident
///   endpoints means *zero* sweep, not a circle.
fn arc_data(cx: f64, cy: f64, r: f64, start: f64, end: f64) -> String {
    let sweep = end - start;
    let point = |angle: f64| (cx + r * angle.cos(), cy + r * angle.sin());
    let (x0, y0) = point(start);
    if sweep <= 0.0 || !sweep.is_finite() {
        // A degenerate sweep is a point; a zero-length arc is still expressible.
        return format!(
            "M {} {} L {} {}",
            fmt_number(x0),
            fmt_number(y0),
            fmt_number(x0),
            fmt_number(y0)
        );
    }
    let radius = fmt_number(r);
    if sweep >= std::f64::consts::TAU - 1e-12 {
        let (xm, ym) = point(start + std::f64::consts::PI);
        return format!(
            "M {x0} {y0} A {radius} {radius} 0 1 1 {xm} {ym} A {radius} {radius} 0 1 1 {x0} {y0}",
            x0 = fmt_number(x0),
            y0 = fmt_number(y0),
            xm = fmt_number(xm),
            ym = fmt_number(ym),
        );
    }
    let (x1, y1) = point(end);
    let large = usize::from(sweep > std::f64::consts::PI);
    format!(
        "M {} {} A {} {} 0 {} 1 {} {}",
        fmt_number(x0),
        fmt_number(y0),
        radius,
        radius,
        large,
        fmt_number(x1),
        fmt_number(y1),
    )
}

/// XML text escaping, for the one thing that is user text: the node name.
fn escape_xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}
