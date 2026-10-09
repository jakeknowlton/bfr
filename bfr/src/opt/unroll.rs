//! Replaces a loop whose trip count is known at compile time with that many
//! copies of its body, within size limits.
//!
//! The trip count is known when the control cell holds a known value at the
//! loop and each trip changes it by the same constant. The pass counts how
//! many trips reach zero, and declines if that takes more than `max_trips`.
//!
//! Examples
//!
//! `[p] = 2; while [p] { write([p+1]); [p] -= 1 }` becomes `[p] = 2; write([p+1]); [p] -= 1; write([p+1]); [p] -= 1`.

use crate::config::Dialect;
use crate::ir::{Block, BlockSite, CellDelta, EffKind, Loop, Node, NodeKind, Program, arith};
use crate::opt::state::{Known, State};
use crate::opt::{Changed, Ctx, Pass};

pub struct LoopUnroll {
    /// Do not unroll loops whose body exceeds this many effects.
    pub max_body_effects: usize,
    /// Do not unroll loops with more than this many trips.
    pub max_trips: u32,
}

impl Default for LoopUnroll {
    fn default() -> Self {
        LoopUnroll {
            max_body_effects: 16,
            max_trips: 64,
        }
    }
}

impl Pass for LoopUnroll {
    fn name(&self) -> &'static str {
        "LoopUnroll"
    }

    fn run(&self, program: &mut Program, ctx: &Ctx<'_>) -> Changed {
        let mut changed = false;
        program.for_each_block_mut(&mut |block, site| {
            changed |= self.unroll_block(block, site, ctx.dialect);
        });
        changed
    }
}

impl LoopUnroll {
    /// Unroll every loop in `block` whose trip count is known from the
    /// block's entry state.
    fn unroll_block(&self, block: &mut Block, site: BlockSite, dialect: &Dialect) -> bool {
        let mut state = State::on_entry(site);
        // Splicing merges neighboring runs, so replacements wait until the
        // walk is done.
        let mut edits: Vec<(usize, Vec<Node>)> = Vec::new();
        for (i, node) in block.nodes().iter().enumerate() {
            match &node.kind {
                NodeKind::Run(run) => state.advance(run, dialect),
                NodeKind::Scan(_) => state.pass_scan(),
                NodeKind::Loop(l) => {
                    if let Known::Value(start) = state.get(0)
                        && start != 0
                        && let Some(trips) = self.trips(l, start, dialect)
                    {
                        let body = l.body.nodes();
                        let copies = (0..trips).flat_map(|_| body.iter().cloned()).collect();
                        edits.push((i, copies));
                    }
                    // Whether unrolled or not, what the loop leaves behind
                    // is the same.
                    state.pass_loop(l);
                }
            }
        }
        let changed = !edits.is_empty();
        for (i, copies) in edits.into_iter().rev() {
            block.splice(i..i + 1, copies);
        }
        changed
    }

    /// How many trips `l` takes from a control cell of `start`, if that is
    /// known and within the limits.
    fn trips(&self, l: &Loop, start: u32, dialect: &Dialect) -> Option<u32> {
        if l.body.op_count() > self.max_body_effects {
            return None;
        }
        let by = control_change(l)?;
        let mut cell = start;
        for trip in 1..=self.max_trips {
            cell = arith::apply_delta(cell, by, dialect);
            if cell == 0 {
                return Some(trip);
            }
        }
        None
    }
}

/// How much one trip of `l` changes the control cell, when that is the same
/// constant every trip. That holds when the body returns the pointer to
/// where it started and only ever adds constants to the control cell.
fn control_change(l: &Loop) -> Option<CellDelta> {
    let mut cursor: isize = 0;
    let mut sum: i64 = 0;
    for node in l.body.nodes() {
        match &node.kind {
            NodeKind::Run(run) => {
                for eff in &run.effects {
                    if eff.kind.writes() != Some(-cursor) {
                        continue;
                    }
                    let EffKind::Add { delta, .. } = eff.kind else {
                        return None;
                    };
                    sum += i64::from(delta);
                }
                cursor += run.shift;
            }
            NodeKind::Loop(inner) => {
                let footprint = inner.footprint()?;
                if footprint.writes.contains(&-cursor) {
                    return None;
                }
            }
            NodeKind::Scan(_) => return None,
        }
    }
    if cursor != 0 {
        return None;
    }
    CellDelta::try_from(sum).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, OptLevel};
    use crate::ir::{lower, print};
    use crate::opt::test_support::*;
    use crate::parser;

    fn unrolled_by(src: &str, pass: &LoopUnroll) -> (Program, Changed) {
        let dialect = Dialect::default();
        let mut program = lower::lower(&parser::parse(src).expect("parses"));
        let changed = pass.run(&mut program, &Ctx::new(&dialect));
        (program, changed)
    }

    fn unrolled(src: &str) -> (Program, Changed) {
        unrolled_by(src, &LoopUnroll::default())
    }

    #[test]
    fn a_known_count_becomes_copies_of_the_body() {
        let (program, changed) = unrolled("++[>.<-]");
        assert!(changed);
        let expected = [
            "[p] += 1\n",
            "[p] += 1\n",
            "write([p+1])\n",
            "[p] -= 1\n",
            "write([p+1])\n",
            "[p] -= 1\n",
        ];
        assert_eq!(print::print(&program), expected.concat());
    }

    #[test]
    fn copies_merge_with_their_neighbors() {
        let (program, _) = unrolled("++[>.<-]+");
        assert_eq!(program.body.len(), 1);
    }

    #[test]
    fn an_even_change_that_reaches_zero_unrolls() {
        let (program, changed) = unrolled("++++[.--]");
        assert!(changed);
        let expected = [
            "[p] += 1\n",
            "[p] += 1\n",
            "[p] += 1\n",
            "[p] += 1\n",
            "write([p])\n",
            "[p] -= 1\n",
            "[p] -= 1\n",
            "write([p])\n",
            "[p] -= 1\n",
            "[p] -= 1\n",
        ];
        assert_eq!(print::print(&program), expected.concat());
    }

    #[test]
    fn an_even_change_that_never_reaches_zero_survives() {
        // From 1, going down by 2 stays odd forever.
        let generous = LoopUnroll {
            max_trips: 1000,
            ..LoopUnroll::default()
        };
        let (_, changed) = unrolled_by("+[.--]", &generous);
        assert!(!changed);
    }

    #[test]
    fn a_change_that_reaches_zero_only_by_wrapping_counts_the_wrap() {
        // From 4, going down by 6 on u8 cells reaches 0 after 86 trips.
        let (_, changed) = unrolled("++++[.------]");
        assert!(!changed);
        let generous = LoopUnroll {
            max_trips: 100,
            ..LoopUnroll::default()
        };
        let (program, changed) = unrolled_by("++++[.------]", &generous);
        assert!(changed);
        let writes = print::print(&program).matches("write").count();
        assert_eq!(writes, 86);
    }

    #[test]
    fn the_trip_limit_declines_long_loops() {
        let tight = LoopUnroll {
            max_trips: 2,
            ..LoopUnroll::default()
        };
        let (_, changed) = unrolled_by("+++[>.<-]", &tight);
        assert!(!changed);
        let (_, changed) = unrolled_by("++[>.<-]", &tight);
        assert!(changed);
    }

    #[test]
    fn the_body_limit_declines_big_bodies() {
        let tight = LoopUnroll {
            max_body_effects: 1,
            ..LoopUnroll::default()
        };
        let (_, changed) = unrolled_by("++[>.<-]", &tight);
        assert!(!changed);
    }

    #[test]
    fn an_unknown_control_cell_survives() {
        let (_, changed) = unrolled(",[>.<-]");
        assert!(!changed);
    }

    #[test]
    fn a_zero_control_cell_is_left_for_dead_code() {
        let (_, changed) = unrolled("[>.<-]");
        assert!(!changed);
    }

    #[test]
    fn a_body_that_moves_the_pointer_survives() {
        let (_, changed) = unrolled("++[>.-]");
        assert!(!changed);
    }

    #[test]
    fn a_body_that_stores_or_reads_the_control_cell_survives() {
        let (_, changed) = unrolled("++[>.<,]");
        assert!(!changed);
        let (_, changed) = unrolled("++[>+<[-]]");
        assert!(!changed);
    }

    #[test]
    fn a_nested_loop_that_leaves_the_control_cell_alone_is_copied() {
        let (program, changed) = unrolled("++[>[-]<-]");
        assert!(changed);
        let expected = [
            "[p] += 1\n",
            "[p] += 1\n",
            "p += 1\n",
            "while [p] {\n",
            "  [p] -= 1\n",
            "}\n",
            "[p-1] -= 1\n",
            "while [p] {\n",
            "  [p] -= 1\n",
            "}\n",
            "[p-1] -= 1\n",
            "p -= 1\n",
        ];
        assert_eq!(print::print(&program), expected.concat());
    }

    #[test]
    fn a_nested_loop_that_writes_the_control_cell_survives() {
        let (_, changed) = unrolled("++[>[-<+>]<-]");
        assert!(!changed);
    }

    #[test]
    fn a_control_cell_known_inside_a_loop_body_unrolls() {
        let (program, changed) = unrolled(",[>[-]++[.-]<,]");
        assert!(changed);
        let expected = [
            "[p] = read()\n",
            "while [p] {\n",
            "  p += 1\n",
            "  while [p] {\n",
            "    [p] -= 1\n",
            "  }\n",
            "  [p] += 1\n",
            "  [p] += 1\n",
            "  write([p])\n",
            "  [p] -= 1\n",
            "  write([p])\n",
            "  [p] -= 1\n",
            "  [p-1] = read()\n",
            "  p -= 1\n",
            "}\n",
        ];
        assert_eq!(print::print(&program), expected.concat());
    }

    #[test]
    fn knowledge_does_not_leak_into_a_loop_body() {
        // The inner `++` lands on a cell the outer body starts not knowing.
        let (_, changed) = unrolled(",[>++[.-]<,]");
        assert!(!changed);
    }

    #[test]
    fn is_idempotent() {
        let dialect = Dialect::default();
        let ctx = Ctx::new(&dialect);
        let pass = LoopUnroll::default();
        let mut program = lower::lower(&parser::parse("++[>.<-]").expect("parses"));
        assert!(pass.run(&mut program, &ctx));
        let settled = program.clone();
        assert!(!pass.run(&mut program, &ctx));
        assert_eq!(program, settled);
    }

    fn with_unroll(pass: LoopUnroll) -> impl Fn(Dialect) -> Config {
        move |dialect| {
            let mut config = Config::new(OptLevel::O0).with_dialect(dialect);
            config.pipeline.push(Box::new(LoopUnroll {
                max_body_effects: pass.max_body_effects,
                max_trips: pass.max_trips,
            }));
            config
        }
    }

    #[test]
    fn output_matches_o0_at_every_width() {
        for src in [
            "++[>.<-]",
            "++++[.--]",
            "++[>[-]<-]",
            ",[>[-]++[.-]<,]",
            "+++[>+++[>+<-]<-]>>.",
        ] {
            assert_matches_o0(src, b"\x02\x00", with_unroll(LoopUnroll::default()));
        }
    }

    #[test]
    fn a_wrapping_count_matches_o0() {
        // Only at u8, where the wrap happens after 86 trips rather than
        // billions.
        let generous = LoopUnroll {
            max_trips: 100,
            ..LoopUnroll::default()
        };
        assert_matches_o0_under(Dialect::default(), "++++[.------]", b"", with_unroll(generous));
    }

    #[test]
    fn faults_match_o0() {
        for src in ["<++[>.<-]", "++[<.>-]", "++[>+[<]<-]"] {
            assert_faults_match_o0(src, with_unroll(LoopUnroll::default()));
        }
    }
}
