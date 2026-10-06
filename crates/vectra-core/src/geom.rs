//! Minimal geometric value types owned by `vectra-core`.
//!
//! Architectural note: `core` owns the *data model* (plain values), while
//! `vectra-geometry` owns *algorithms* (arcs, regions, tessellation). This
//! keeps the crate DAG acyclic: `geometry → core`, never the reverse.

use serde::{Deserialize, Serialize};

/// A 2D point in document units.
///
/// Used for path vertices and control points. Scalar primitives
/// (Rectangle/Circle/Arc) intentionally decompose into `f64` parameters
/// (`x`, `y`, `width`, …) because constraints (MES §9) and expressions
/// (MES §7) are scalar systems — addressing `Rectangle.width` must be a
/// first-class operation, not a vector swizzle.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point2 {
    pub x: f64,
    pub y: f64,
}

impl Point2 {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// RGBA color value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const BLACK: Self = Self {
        r: 0,
        g: 0,
        b: 0,
        a: 255,
    };
    pub const WHITE: Self = Self {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    };
    pub const TRANSPARENT: Self = Self {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };

    pub fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Parse `#rrggbb` or `#rrggbbaa`. Returns `None` on malformed input.
    pub fn from_hex(hex: &str) -> Option<Self> {
        let h = hex.strip_prefix('#').unwrap_or(hex);
        let (r, g, b, a) = match h.len() {
            6 => (
                u8::from_str_radix(&h[0..2], 16).ok()?,
                u8::from_str_radix(&h[2..4], 16).ok()?,
                u8::from_str_radix(&h[4..6], 16).ok()?,
                255,
            ),
            8 => (
                u8::from_str_radix(&h[0..2], 16).ok()?,
                u8::from_str_radix(&h[2..4], 16).ok()?,
                u8::from_str_radix(&h[4..6], 16).ok()?,
                u8::from_str_radix(&h[6..8], 16).ok()?,
            ),
            _ => return None,
        };
        Some(Self { r, g, b, a })
    }

    /// Straight (non-premultiplied) interpolation, `t = 0` returning `self`.
    ///
    /// Channels interpolate in `f64` and round at the end: a gradient with a
    /// hundred stops must not accumulate a whole byte of error per stop, and
    /// `0.5` of black-to-white is `(128,128,128)` — what a designer sees in every
    /// other tool, and what `Color::rgb(128,128,128)` already means here.
    pub fn lerp(self, other: Self, t: f64) -> Self {
        let t = t.clamp(0.0, 1.0);
        let mix = |a: u8, b: u8| -> u8 {
            let value = a as f64 + (b as f64 - a as f64) * t;
            value.round().clamp(0.0, 255.0) as u8
        };
        Self {
            r: mix(self.r, other.r),
            g: mix(self.g, other.g),
            b: mix(self.b, other.b),
            a: mix(self.a, other.a),
        }
    }

    /// The same colour with alpha replaced. Used when a layer's opacity is folded
    /// into its paint for a single draw.
    pub fn with_alpha(self, alpha: u8) -> Self {
        Self { a: alpha, ..self }
    }

    /// Is every channel zero alpha?
    pub fn is_transparent(&self) -> bool {
        self.a == 0
    }

    pub fn to_hex(&self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_hex_roundtrip() {
        let c = Color::from_hex("#ff8040").unwrap();
        assert_eq!(c, Color::rgb(0xff, 0x80, 0x40));
        assert_eq!(c.to_hex(), "#ff8040");

        let t = Color::from_hex("#00ff0080").unwrap();
        assert_eq!(t.a, 0x80);
        assert_eq!(t.to_hex(), "#00ff0080");

        assert!(Color::from_hex("not-a-color").is_none());
    }
}
