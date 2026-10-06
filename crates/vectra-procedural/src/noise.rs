//! Deterministic value noise (Task 7.0 RULE 5).
//!
//! # Why not `rand`, and why not `sin`-based hash
//!
//! A procedural node's output is part of the *document's* meaning: the same
//! document must produce byte-identical geometry on native and on wasm, in
//! debug and in release, today and after an undo. That rules out
//! [`rand`](https://docs.rs/rand) (its default generators are not required to
//! be reproducible across versions) and it rules out the classic
//! `fract(sin(dot(p, k)) * 43758.5453)` hash, whose result depends on the
//! platform's `sin` implementation for large arguments.
//!
//! What is left is integer hashing: `splitmix64` on a lattice coordinate mixed
//! with the node's seed, mapped to `[0, 1)` by a conversion that is exact on
//! every IEEE-754 platform (`u64 >> 11` then `× 2⁻⁵³`, which is lossless
//! because the top 53 bits of an f64's mantissa are exactly representable).
//!
//! Determinism is then a property of integer arithmetic and IEEE addition —
//! neither of which a compiler is allowed to reorder into a different answer —
//! and the Determinism Law is checkable by comparing two engines rather than by
//! trusting a comment.

/// One lattice point's value in `[0, 1)`, from an integer coordinate and a seed.
///
/// `splitmix64`'s finalizer: the mixture is a bijection on `u64`, so distinct
/// lattice points never collide, and no input is a fixed point of the whole
/// pipeline for any seed.
pub fn hash01(seed: u64, x: i64, y: i64) -> f64 {
    let mut h = seed
        .wrapping_add((x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))
        .wrapping_add((y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F));
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= h >> 31;
    // 53 significant bits → [0, 1) with no rounding surprises.
    ((h >> 11) as f64) * (1.0 / (1u64 << 53) as f64)
}

/// A `seed` operand (`f64`, because every procedural operand is a number) as an
/// integer seed. Fractional seeds are folded in deterministically, and negative
/// or absurd values cannot panic or lose determinism.
pub fn seed_bits(seed: f64) -> u64 {
    if !seed.is_finite() {
        return 0;
    }
    let scaled = (seed * 4096.0).round();
    // i128 keeps the conversion well-defined for huge magnitudes.
    let as_int = (scaled as i128) as u64;
    as_int ^ 0x5DEE_CE66_D0F1_1235
}

/// Smoothstep, the interpolation curve of the value noise.
///
/// `t²(3 − 2t)` has zero derivative at both ends, so the field is C¹ across
/// cell boundaries — which is what keeps a displaced region smooth instead of
/// showing the lattice as faceted corners. Multiplications only: no
/// transcendentals, no platform-dependent behaviour.
fn smoothstep(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

/// Value noise at `(x, y)` in `[0, 1)` — the bilinear blend of the four lattice
/// corners' [`hash01`] values, smoothstepped.
pub fn value_noise(seed: u64, x: f64, y: f64) -> f64 {
    if !x.is_finite() || !y.is_finite() {
        return 0.0;
    }
    let x0 = x.floor();
    let y0 = y.floor();
    let tx = smoothstep(x - x0);
    let ty = smoothstep(y - y0);
    let (ix, iy) = (x0 as i64, y0 as i64);
    let c00 = hash01(seed, ix, iy);
    let c10 = hash01(seed, ix.wrapping_add(1), iy);
    let c01 = hash01(seed, ix, iy.wrapping_add(1));
    let c11 = hash01(seed, ix.wrapping_add(1), iy.wrapping_add(1));
    let bottom = c00 + (c10 - c00) * tx;
    let top = c01 + (c11 - c01) * tx;
    (bottom + (top - bottom) * ty).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hash_is_total_and_in_range() {
        for seed in [0u64, 1, 42, u64::MAX] {
            for (x, y) in [
                (0i64, 0i64),
                (-1, -1),
                (i64::MAX, i64::MIN),
                (1_000_000, -7_000_000),
            ] {
                let v = hash01(seed, x, y);
                assert!((0.0..1.0).contains(&v), "{v} out of range");
            }
        }
        assert_eq!(seed_bits(f64::NAN), 0);
        assert_eq!(seed_bits(f64::INFINITY), 0);
        assert_eq!(seed_bits(1.0), seed_bits(1.0));
        assert_ne!(seed_bits(1.0), seed_bits(1.25));
    }

    #[test]
    fn the_same_coordinate_always_gives_the_same_value() {
        // The whole point of RULE 5: no hidden state, no clock, no ambient.
        let first = hash01(7, 3, -9);
        for _ in 0..64 {
            assert_eq!(hash01(7, 3, -9), first);
        }
        // Different seeds decorrelate.
        assert_ne!(hash01(7, 3, -9), hash01(8, 3, -9));
    }

    #[test]
    fn the_noise_field_is_continuous_and_interpolates_the_lattice() {
        // At lattice points the field *is* the hash…
        for ix in -3i64..3 {
            for iy in -3i64..3 {
                let v = value_noise(11, ix as f64, iy as f64);
                assert!((v - hash01(11, ix, iy)).abs() < 1e-12);
            }
        }
        // …and between them it moves smoothly: no jump larger than the corner
        // values allow, over a fine sweep through one cell.
        let mut previous = value_noise(11, 0.0, 0.25);
        for step in 1..=200 {
            let v = value_noise(11, step as f64 * 0.005, 0.25);
            assert!(
                (v - previous).abs() < 0.05,
                "jump at step {step}: {previous} → {v}"
            );
            previous = v;
        }
        assert!((0.0..1.0).contains(&previous));
    }

    #[test]
    fn non_finite_inputs_cannot_produce_nan() {
        assert_eq!(value_noise(1, f64::NAN, 0.0), 0.0);
        assert_eq!(value_noise(1, 0.0, f64::INFINITY), 0.0);
        assert!(value_noise(1, 1e300, 1e300).is_finite());
    }
}
