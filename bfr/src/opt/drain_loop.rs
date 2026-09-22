//! Rewrites loops that step their control cell by a terminating constant into direct arithmetic,
//! turning O(cell value) trips into O(1).
//!
//! Examples
//!
//! `[-]` becomes `[p] = 0`.
//! `[->+<]` becomes `[p+1] += [p]; [p] = 0`.

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
                let LoopShape::Drain { step, targets } = l.shape() else {
                    return None;
                };
                let trip_factor = arith::drain_factor(step, ctx.dialect)?;

                // One scaled add per target, then the control cell is zeroed
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
                effects.push(Eff::new(EffKind::Set { at: 0, value: 0 }, span));
                let run = Node::new(NodeKind::Run(Run { effects, shift: 0 }), span);

                Some(vec![if targets.is_empty() {
                    // Just set the current cell to 0
                    run
                } else {
                    // This loop always runs at most once, more like an `if`
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
        assert_eq!(print::print(&program), "[p] = 0\n");
    }

    #[test]
    fn the_bare_store_merges_with_its_neighbors() {
        // Composition of the `+`s is Normalize's job; the merge into one
        // run is the block edit's.
        let (program, _) = drained("+++[-]");
        assert_eq!(program.body.len(), 1);
        assert_eq!(
            print::print(&program),
            "[p] += 1\n[p] += 1\n[p] += 1\n[p] = 0\n"
        );
    }

    #[test]
    fn a_transfer_keeps_the_run_once_shell() {
        let (program, changed) = drained("[->+<]");
        assert!(changed);
        let expected = ["while [p] {\n", "  [p+1] += [p]\n", "  [p] = 0\n", "}\n"];
        assert_eq!(print::print(&program), expected.concat());
    }

    #[test]
    fn a_fan_out_gets_one_scaled_add_per_target() {
        let (program, _) = drained("[->+>+<<]");
        let expected = [
            "while [p] {\n",
            "  [p+1] += [p]\n",
            "  [p+2] += [p]\n",
            "  [p] = 0\n",
            "}\n",
        ];
        assert_eq!(print::print(&program), expected.concat());
    }

    #[test]
    fn per_iteration_deltas_become_factors() {
        let (program, _) = drained("[->>+++<<]");
        let expected = [
            "while [p] {\n",
            "  [p+2] += [p] * 3\n",
            "  [p] = 0\n",
            "}\n",
        ];
        assert_eq!(print::print(&program), expected.concat());
    }

    #[test]
    fn an_odd_step_drains_through_the_modular_inverse() {
        let (program, _) = drained("[--->+<]");
        let expected = [
            "while [p] {\n",
            "  [p+1] += [p] * 171\n",
            "  [p] = 0\n",
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
            "while [p] {\n  [p] -= 1\n  [p] -= 1\n}\n"
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
        assert_eq!(print::print(&program), "while [p] {\n  [p] = 0\n}\n");
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

    #[test]
    fn output_matches_o0() {
        // 3 * 4, a copy, an odd-step drain, and a zero-trip transfer.
        for src in [
            "+++[->++++<]>.",
            "++[->+>+<<]>.>.",
            "+++++[--->+<]>.",
            ">[->+<]<.",
        ] {
            let o0 = crate::run_to_vec(src, &Config::new(OptLevel::O0), b"").expect("runs");
            let mut config = Config::new(OptLevel::O0);
            config.pipeline.push(Box::new(DrainLoop));
            let drained = crate::run_to_vec(src, &config, b"").expect("runs");
            assert_eq!(drained, o0, "mismatch for {src}");
        }
    }
}
