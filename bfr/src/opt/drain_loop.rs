//! Rewrites a loop that drains its control cell to zero by a constant each
//! trip, adding constants to other cells as it goes, into one scaled add per
//! target and a clear of the control cell. What took one trip per unit of
//! the control cell becomes straight-line arithmetic.
//!
//! Examples
//!
//! `[-]` becomes `[0] = 0`.
//! `[->+<]` becomes `[+1] += [0]; [0] = 0`.

use crate::ir::{Block, Eff, EffKind, Loop, LoopShape, Node, NodeKind, Program, Run, arith};
use crate::opt::{Changed, Ctx, Pass};

pub struct DrainLoop;

impl Pass for DrainLoop {
    fn name(&self) -> &'static str {
        "DrainLoop"
    }

    fn run(&self, program: &mut Program, ctx: &Ctx<'_>) -> Changed {
        let mut changed = false;
        program.for_each_block_mut(&mut |block, _| {
            changed |= block.try_replace_loops(|l, span| {
                let LoopShape::Drain { by, targets } = l.shape() else {
                    return None;
                };
                let trip_factor = arith::drain_factor(by, ctx.dialect)?;

                // One scaled add per target, then clear the control cell
                let mut effects: Vec<Eff> = targets
                    .iter()
                    .map(|&(at, per_trip)| {
                        Eff::new(
                            EffKind::AddScaled {
                                at,
                                from: 0,
                                factor: arith::target_factor(per_trip, trip_factor, ctx.dialect),
                            },
                            span,
                        )
                    })
                    .collect();
                effects.push(Eff::new(EffKind::Store { at: 0, value: 0 }, span));
                let run = Node::new(NodeKind::Run(Run { effects, shift: 0 }), span);

                Some(vec![if targets.is_empty() {
                    // No targets, so the loop is only a clear
                    run
                } else {
                    // Keep a shell around the run. In the original, a zero
                    // control cell never touched the targets, so they may be
                    // off the tape.
                    let body = Block::from_nodes(vec![run]);
                    Node::new(NodeKind::Loop(Loop { body }), span)
                }])
            });
        });
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Dialect, OptLevel};
    use crate::ir::{lower, print};
    use crate::opt::test_support::{assert_matches_o0, assert_matches_o0_under};
    use crate::parser;

    fn drained(src: &str) -> (Program, Changed) {
        let dialect = Dialect::default();
        let mut program = lower::lower(&parser::parse(src).expect("parses"));
        let changed = DrainLoop.run(&mut program, &Ctx::new(&dialect));
        (program, changed)
    }

    #[test]
    fn a_clear_loop_becomes_a_bare_store() {
        let (program, changed) = drained("[-]");
        assert!(changed);
        assert_eq!(print::print(&program), "[0] = 0\n");
    }

    #[test]
    fn the_bare_store_merges_with_its_neighbors() {
        // Composing the `+`s is Normalize's job. Merging into one run is
        // done by the block edit itself.
        let (program, _) = drained("+++[-]");
        assert_eq!(program.body.len(), 1);
        assert_eq!(
            print::print(&program),
            "[0] += 1\n[0] += 1\n[0] += 1\n[0] = 0\n"
        );
    }

    #[test]
    fn a_transfer_keeps_its_shell() {
        let (program, changed) = drained("[->+<]");
        assert!(changed);
        let expected = ["loop {\n", "  [+1] += [0]\n", "  [0] = 0\n", "}\n"];
        assert_eq!(print::print(&program), expected.concat());
    }

    #[test]
    fn a_fan_out_gets_one_scaled_add_per_target() {
        let (program, _) = drained("[->+>+<<]");
        let expected = [
            "loop {\n",
            "  [+1] += [0]\n",
            "  [+2] += [0]\n",
            "  [0] = 0\n",
            "}\n",
        ];
        assert_eq!(print::print(&program), expected.concat());
    }

    #[test]
    fn per_trip_deltas_become_factors() {
        let (program, _) = drained("[->>+++<<]");
        let expected = [
            "loop {\n",
            "  [+2] += [0] * 3\n",
            "  [0] = 0\n",
            "}\n",
        ];
        assert_eq!(print::print(&program), expected.concat());
    }

    #[test]
    fn an_odd_step_drains_through_the_modular_inverse() {
        let (program, _) = drained("[--->+<]");
        let expected = [
            "loop {\n",
            "  [+1] += [0] * 171\n",
            "  [0] = 0\n",
            "}\n",
        ];
        assert_eq!(print::print(&program), expected.concat());
    }

    #[test]
    fn an_even_step_survives_verbatim() {
        let (program, changed) = drained("[--]");
        assert!(!changed);
        assert_eq!(
            print::print(&program),
            "loop {\n  [0] += -1\n  [0] += -1\n}\n"
        );
    }

    #[test]
    fn io_and_shifting_bodies_survive() {
        let (_, changed) = drained("[.-]");
        assert!(!changed);
        let (_, changed) = drained("[->+]");
        assert!(!changed);
        let (_, changed) = drained("[,]");
        assert!(!changed);
    }

    #[test]
    fn nested_drains_rewrite_innermost_first() {
        let (program, changed) = drained("[[-]]");
        assert!(changed);
        assert_eq!(print::print(&program), "loop {\n  [0] = 0\n}\n");
    }

    #[test]
    fn is_idempotent() {
        let dialect = Dialect::default();
        let ctx = Ctx::new(&dialect);
        let mut program = lower::lower(&parser::parse("[->+<][-]").expect("parses"));
        assert!(DrainLoop.run(&mut program, &ctx));
        let settled = program.clone();
        assert!(!DrainLoop.run(&mut program, &ctx));
        assert_eq!(program, settled);
    }

    fn with_drain_loop(dialect: Dialect) -> Config {
        let mut config = Config::new(OptLevel::O0).with_dialect(dialect);
        config.pipeline.push(Box::new(DrainLoop));
        config
    }

    #[test]
    fn output_matches_o0() {
        // 3 * 4, a copy, and a zero-trip transfer.
        for src in ["+++[->++++<]>.", "++[->+>+<<]>.>.", ">[->+<]<."] {
            assert_matches_o0(src, b"", with_drain_loop);
        }
    }

    #[test]
    fn an_odd_step_drain_matches_o0() {
        // 5 going down by 3 only reaches 0 by wrapping. Only at u8, where
        // that takes 171 trips rather than billions.
        assert_matches_o0_under(Dialect::default(), "+++++[--->+<]>.", b"", with_drain_loop);
    }
}
