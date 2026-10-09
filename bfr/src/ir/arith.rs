//! Arithmetic for the folding passes.

use crate::config::Dialect;
use crate::ir::{Cell, CellDelta};

/// Truncate an exact `i64` result to the cell width.
pub fn mask(value: i64, dialect: &Dialect) -> Cell {
    (value as u32) & dialect.cell_width.mask()
}

/// Apply a signed delta to a masked cell value.
pub fn apply_delta(value: Cell, delta: CellDelta, dialect: &Dialect) -> Cell {
    mask(i64::from(value) + i64::from(delta), dialect)
}

/// Add two changes to the same cell, reducing the sum only when it does
/// not fit in a [`CellDelta`].
///
/// # Examples
///
/// On u8 cells, `200` and `100` fuse to `300`. But `i32::MAX` and `1` sum
/// to `2^31`, too big to store and an exact multiple of 256, so they fuse
/// to `0`.
pub fn fuse_deltas(a: CellDelta, b: CellDelta, dialect: &Dialect) -> CellDelta {
    let sum = i64::from(a) + i64::from(b);
    CellDelta::try_from(sum).unwrap_or_else(|_| reduce_delta(sum, dialect))
}

/// Rewrite `value` as the change closest to zero that leaves the cell in
/// the same state.
///
/// # Examples
///
/// On u8 cells, `+300` becomes `+44`, and `+200` becomes `-56`.
fn reduce_delta(value: i64, dialect: &Dialect) -> CellDelta {
    let modulus = i64::from(dialect.cell_width.mask()) + 1;
    let mut r = value.rem_euclid(modulus);
    if r >= modulus / 2 {
        r -= modulus;
    }
    r as CellDelta
}

/// The value that, multiplied by `value`, gives 1 in a cell of the
/// dialect's width. Dividing by `value` is the same as multiplying by
/// this. Only odd values have an inverse, so even values get `None`.
///
/// # Examples
///
/// On u8 cells, `3 * 171 == 513`, which wraps to `1`, so `171` is an
/// inverse of `3` (and, in fact, is the only inverse of `3`).
pub fn modular_inverse(value: u32, dialect: &Dialect) -> Option<u32> {
    if value.is_multiple_of(2) {
        return None;
    }
    // Newton-Raphson iteration for the inverse of `v = value`, which over
    // the 2-adics is Hensel lifting. I found this method in this paper:
    // J-G. Dumas, "On Newton-Raphson Iteration for Multiplicative Inverses
    // Modulo Prime Powers", IEEE Trans. Computers 63(8), 2014,
    // pp. 2106-2109 (arXiv:1209.6626).
    //
    // The goal is to find some x such that v*x ≡ 1 (mod 2^32). `e` defined
    // as `e = 1 - v*x` is the gap between what we have and the 1 we want,
    // so `e == 0` means that `x` is the inverse. Two numbers differing by a
    // multiple of 2^k share their lower k bits, so an `e` divisible by 2^k
    // means the lower k bits of `v*x` already read as 1. A short proof for
    // this case:
    //
    //   1. `x = v` starts `e` at a multiple of 8. This is because
    //      `v*v ≡ 1 (mod 8)` for any odd v (try it!).
    //   2. One iteration step `x = x(2 - v*x)` squares `e`, since
    //      `1 - v*x(2 - v*x)` refactors to `(1 - v*x)^2`.
    //   3. If `e` is a multiple of 2^k then `e*e` is a multiple of 2^(2k),
    //      since `(2^k * m)^2 = 2^(2k) * m^2`.
    //
    // Four steps therefore take `e` from a multiple of 2^3 to 2^48
    // (3 -> 6 -> 12 -> 24 -> 48), and any multiple of 2^48 is one of
    // 2^32, so `e ≡ 0 (mod 2^32)`. So `x` is exactly the inverse after 4
    // iterations.
    let mut x = value;
    for _ in 0..4 {
        x = x.wrapping_mul(2u32.wrapping_sub(value.wrapping_mul(x)));
    }
    // An inverse mod 2^32 is also an inverse mod any narrower width, so a
    // narrower cell just truncates.
    Some(mask(i64::from(x), dialect))
}

/// The multiplier from the control cell's starting value to the number
/// of trips a loop takes. From a control cell of `start`, the loop runs
/// `factor * start` times, wrapping at the cell width. `by` is what one
/// trip adds to the control cell.
///
/// An even `by` gets `None`: it can jump over zero and loop forever, so
/// such loops must stay loops.
///
/// # Examples
///
/// `[-]` drains by `-1` and runs `start` times, so its factor is `1`.
/// On u8 cells, `[---]` drains by `-3`, so its factor is `171`. In this
/// case, a control cell of `1` takes `171` trips to reach zero.
pub fn drain_factor(by: CellDelta, dialect: &Dialect) -> Option<Cell> {
    modular_inverse(mask(-i64::from(by), dialect), dialect)
}

/// The multiplier from the control cell's starting value to the total one
/// target cell gains over the whole loop. From a control cell of
/// `start`, the target gains `factor * start`. `per_trip` is what one
/// trip adds to the target, and `trip_factor` is the factor from
/// [`drain_factor`].
///
/// # Examples
///
/// `[->+++<]` adds `3` to its target on the right each trip and runs
/// `start` trips, so the target gains `3 * start` and `3` is returned.
/// On u8 cells, `[--->++<]` adds `2` per trip, and draining by `-3` runs
/// the loop `171 * start` trips, so the target gains `2 * 171 * start`
/// and `342` (or `2 * 171`) is returned for any `start`.
pub fn target_factor(per_trip: CellDelta, trip_factor: Cell, dialect: &Dialect) -> CellDelta {
    let product = i64::from(per_trip) * i64::from(trip_factor);
    CellDelta::try_from(product).unwrap_or_else(|_| reduce_delta(product, dialect))
}

/// The change a scaled add makes when its source cell holds `from`.
///
/// # Examples
///
/// `[+1] += [0] * 4` with `[0]` holding `3` adds `12`. On u8 cells,
/// `[+1] += [0] * 171` with `[0]` holding `3` adds `513`, which is `1`.
pub fn scaled_delta(from: Cell, factor: CellDelta, dialect: &Dialect) -> CellDelta {
    let product = i64::from(from) * i64::from(factor);
    CellDelta::try_from(product).unwrap_or_else(|_| reduce_delta(product, dialect))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CellWidth;

    fn dialect(cell_width: CellWidth) -> Dialect {
        Dialect {
            cell_width,
            ..Dialect::default()
        }
    }

    #[test]
    fn mask_truncates_to_the_width() {
        assert_eq!(mask(256, &dialect(CellWidth::U8)), 0);
        assert_eq!(mask(-1, &dialect(CellWidth::U8)), 255);
        assert_eq!(mask(-1, &dialect(CellWidth::U16)), 0xffff);
        assert_eq!(mask(1 << 32, &dialect(CellWidth::U32)), 0);
    }

    #[test]
    fn apply_delta_wraps_at_the_width() {
        assert_eq!(apply_delta(255, 1, &dialect(CellWidth::U8)), 0);
        assert_eq!(apply_delta(255, 1, &dialect(CellWidth::U16)), 256);
        assert_eq!(apply_delta(0, -1, &dialect(CellWidth::U32)), u32::MAX);
    }

    #[test]
    fn fuse_deltas_keeps_the_exact_sum_when_it_fits() {
        assert_eq!(fuse_deltas(200, 100, &dialect(CellWidth::U8)), 300);
        assert_eq!(fuse_deltas(2, -3, &dialect(CellWidth::U8)), -1);
    }

    #[test]
    fn fuse_deltas_reduces_an_overflowing_sum() {
        // 2 * (2^31 - 1) = 2^32 - 2, which is -2 mod 2^32.
        assert_eq!(
            fuse_deltas(i32::MAX, i32::MAX, &dialect(CellWidth::U32)),
            -2
        );
        // 2^31 is 0 mod 256.
        assert_eq!(fuse_deltas(i32::MAX, 1, &dialect(CellWidth::U8)), 0);
    }

    #[test]
    fn modular_inverse_of_odd_values() {
        let u8 = dialect(CellWidth::U8);
        assert_eq!(modular_inverse(1, &u8), Some(1));
        // 3 * 171 = 513 = 2 * 256 + 1.
        assert_eq!(modular_inverse(3, &u8), Some(171));
        assert_eq!(modular_inverse(255, &u8), Some(255));
        let inv = modular_inverse(3, &dialect(CellWidth::U32)).expect("3 is odd");
        assert_eq!(3u32.wrapping_mul(inv), 1);
        let inv = modular_inverse(0xbeef, &dialect(CellWidth::U16)).expect("odd");
        assert_eq!(0xbeefu32.wrapping_mul(inv) & 0xffff, 1);
    }

    #[test]
    fn modular_inverse_declines_even_values() {
        assert_eq!(modular_inverse(0, &dialect(CellWidth::U8)), None);
        assert_eq!(modular_inverse(2, &dialect(CellWidth::U32)), None);
    }

    #[test]
    fn drain_factor_inverts_the_negated_step() {
        let d = dialect(CellWidth::U8);
        assert_eq!(drain_factor(-1, &d), Some(1));
        assert_eq!(drain_factor(-3, &d), Some(171));
        // `[+]` drains too: trips = -initial = 255 * initial mod 256.
        assert_eq!(drain_factor(1, &d), Some(255));
        assert_eq!(drain_factor(-2, &d), None);
        assert_eq!(drain_factor(0, &d), None);
    }

    #[test]
    fn scale_keeps_the_exact_product_when_it_fits() {
        let d = dialect(CellWidth::U8);
        assert_eq!(target_factor(1, 1, &d), 1);
        assert_eq!(target_factor(3, 1, &d), 3);
        assert_eq!(target_factor(1, 171, &d), 171);
        assert_eq!(target_factor(-1, 1, &d), -1);
    }

    #[test]
    fn scaled_delta_keeps_the_exact_product_when_it_fits() {
        let d = dialect(CellWidth::U8);
        assert_eq!(scaled_delta(3, 4, &d), 12);
        assert_eq!(scaled_delta(3, -1, &d), -3);
        // 3 * 171 = 513 stays exact. The cell wraps it to 1 when applied.
        assert_eq!(scaled_delta(3, 171, &d), 513);
        assert_eq!(apply_delta(0, scaled_delta(3, 171, &d), &d), 1);
    }

    #[test]
    fn scaled_delta_reduces_an_overflowing_product() {
        let d = dialect(CellWidth::U32);
        assert_eq!(scaled_delta(u32::MAX, 2, &d), -2);
    }

    #[test]
    fn scale_reduces_an_overflowing_product() {
        let d = dialect(CellWidth::U32);
        // 3^-1 mod 2^32 is 2863311531, and times 2 wraps into i32 range.
        let trip_factor = modular_inverse(3, &d).expect("odd");
        let product = i64::from(trip_factor) * 2;
        let expected = (product - (1i64 << 32)) as i32;
        assert_eq!(target_factor(2, trip_factor, &d), expected);
    }
}
