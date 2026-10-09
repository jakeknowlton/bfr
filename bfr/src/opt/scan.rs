//! Rewrites loops that only move the pointer into a scan.
//!
//! Examples
//!
//! `[>]` becomes `scan p += 1`.
//! `[>>>]` becomes `scan p += 3`.

use crate::ir::{LoopShape, Node, NodeKind, Program, Scan};
use crate::opt::{Changed, Ctx, Pass};

pub struct ScanLoop;

impl Pass for ScanLoop {
    fn name(&self) -> &'static str {
        "ScanLoop"
    }

    fn run(&self, program: &mut Program, _ctx: &Ctx<'_>) -> Changed {
        let mut changed = false;
        program.for_each_block_mut(&mut |block, _| {
            changed |= block.try_replace_loops(|l, span| {
                let LoopShape::Scan { stride } = l.shape() else {
                    return None;
                };
                Some(vec![Node::new(NodeKind::Scan(Scan { stride }), span)])
            });
        });
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Dialect, OptLevel};
    use crate::error::Span;
    use crate::ir::{lower, print};
    use crate::parser;

    fn scanned(src: &str) -> (Program, Changed) {
        let dialect = Dialect::default();
        let mut program = lower::lower(&parser::parse(src).expect("parses"));
        let changed = ScanLoop.run(&mut program, &Ctx::new(&dialect));
        (program, changed)
    }

    #[test]
    fn a_move_only_loop_becomes_a_scan() {
        let (program, changed) = scanned("[>]");
        assert!(changed);
        assert_eq!(print::print(&program), "scan p += 1\n");
    }

    #[test]
    fn the_stride_is_the_net_movement() {
        let (program, _) = scanned("[>>>]");
        assert_eq!(print::print(&program), "scan p += 3\n");
        let (program, _) = scanned("[<<]");
        assert_eq!(print::print(&program), "scan p -= 2\n");
        let (program, _) = scanned("[>><]");
        assert_eq!(print::print(&program), "scan p += 1\n");
    }

    #[test]
    fn bodies_that_touch_a_cell_survive() {
        for src in ["[>+]", "[>.]", "[>,]", "[-]", "[->+<]"] {
            let (_, changed) = scanned(src);
            assert!(!changed, "{src} should not become a scan");
        }
    }

    #[test]
    fn empty_and_balanced_bodies_survive() {
        let (_, changed) = scanned("[]");
        assert!(!changed);
        let (_, changed) = scanned("[><]");
        assert!(!changed);
    }

    #[test]
    fn nested_scans_rewrite_innermost_first() {
        let (program, changed) = scanned("[[>]]");
        assert!(changed);
        assert_eq!(print::print(&program), "while [p] {\n  scan p += 1\n}\n");
    }

    #[test]
    fn the_scan_keeps_the_loop_span() {
        let (program, _) = scanned("+[>]");
        // `[>]` spans bytes 1..4.
        assert_eq!(program.body.nodes()[1].span, Span::new(1, 4));
    }

    #[test]
    fn the_scan_separates_its_neighbors() {
        let (program, _) = scanned("+[>]+");
        assert_eq!(program.body.len(), 3);
        assert_eq!(print::print(&program), "[p] += 1\nscan p += 1\n[p] += 1\n");
    }

    #[test]
    fn is_idempotent() {
        let dialect = Dialect::default();
        let ctx = Ctx::new(&dialect);
        let mut program = lower::lower(&parser::parse("+[>][<<]").expect("parses"));
        assert!(ScanLoop.run(&mut program, &ctx));
        let settled = program.clone();
        assert!(!ScanLoop.run(&mut program, &ctx));
        assert_eq!(program, settled);
    }

    fn with_scan_loop() -> Config {
        let mut config = Config::new(OptLevel::O0);
        config.pipeline.push(Box::new(ScanLoop));
        config
    }

    #[test]
    fn output_matches_o0() {
        // Past three cells, a stride of two, a zero-trip scan, and leftward.
        for src in ["+>+>+<<[>]<.", "+>>+>>+<<<<[>>]<<.", "[>]+.", ">>+>+<[<]>."] {
            let o0 = crate::run_to_vec(src, &Config::new(OptLevel::O0), b"").expect("runs");
            let scanned = crate::run_to_vec(src, &with_scan_loop(), b"").expect("runs");
            assert_eq!(scanned, o0, "mismatch for {src}");
        }
    }

    #[test]
    fn faults_match_o0() {
        // A scan checks every cell it lands on, as the loop test did.
        let src = "+[<]";
        let o0 = crate::run_to_vec(src, &Config::new(OptLevel::O0), b"").expect_err("faults");
        let scanned = crate::run_to_vec(src, &with_scan_loop(), b"").expect_err("faults");
        assert_eq!(scanned, o0);
    }
}
