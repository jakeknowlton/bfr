//! Checks shared by the tests of the passes: an optimized program has to
//! behave exactly like the same program at O0.

use crate::config::{CellWidth, Config, Dialect, OptLevel};

/// Assert that `src` with `input` produces the same output, or the same
/// fault, under `optimized(dialect)` as at O0 under `dialect`.
pub fn assert_matches_o0_under(
    dialect: Dialect,
    src: &str,
    input: &[u8],
    optimized: impl Fn(Dialect) -> Config,
) {
    let o0 = Config::new(OptLevel::O0).with_dialect(dialect);
    assert_eq!(
        crate::run_to_vec(src, &optimized(dialect), input),
        crate::run_to_vec(src, &o0, input),
        "mismatch for {src} under {dialect:?}"
    );
}

/// [`assert_matches_o0_under`] at every cell width.
pub fn assert_matches_o0(src: &str, input: &[u8], optimized: impl Fn(Dialect) -> Config) {
    for cell_width in [CellWidth::U8, CellWidth::U16, CellWidth::U32] {
        let dialect = Dialect {
            cell_width,
            ..Dialect::default()
        };
        assert_matches_o0_under(dialect, src, input, &optimized);
    }
}

/// [`assert_matches_o0`] for a program that faults, checking that it does.
/// The tape is short, so running off its end is quick.
pub fn assert_faults_match_o0(src: &str, optimized: impl Fn(Dialect) -> Config) {
    for cell_width in [CellWidth::U8, CellWidth::U16, CellWidth::U32] {
        let dialect = Dialect {
            cell_width,
            tape_cells: 64,
            ..Dialect::default()
        };
        let o0 = Config::new(OptLevel::O0).with_dialect(dialect);
        assert!(crate::run_to_vec(src, &o0, b"").is_err(), "{src} should fault");
        assert_matches_o0_under(dialect, src, b"", &optimized);
    }
}
