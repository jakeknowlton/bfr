//! The optimization pipeline.

pub mod const_fold;
pub mod dead_code;
pub mod drain_loop;
pub mod normalize;
pub mod partial_eval;
pub mod scan;
mod state;
#[cfg(test)]
pub mod test_support;
pub mod unroll;

use crate::config::{Dialect, OptLevel};
use crate::ir::Program;

/// Whether a pass rewrote anything.
pub type Changed = bool;

/// Read-only context handed to every pass.
#[derive(Debug, Clone, Copy)]
pub struct Ctx<'a> {
    pub dialect: &'a Dialect,
}

impl<'a> Ctx<'a> {
    pub fn new(dialect: &'a Dialect) -> Self {
        Ctx { dialect }
    }
}

/// An IR-to-IR transformation. Object-safe so library consumers can supply
/// their own passes.
pub trait Pass {
    /// Stable identifier, unique within a pipeline.
    fn name(&self) -> &'static str;

    /// Rewrite `program` in place. Returns whether anything changed.
    fn run(&self, program: &mut Program, ctx: &Ctx<'_>) -> Changed;
}

/// An ordered list of passes, run to a fixed point.
pub struct Pipeline {
    passes: Vec<Box<dyn Pass>>,
    /// Upper bound on sweeps, so a pass that never settles cannot spin
    /// forever.
    pub max_sweeps: u32,
}

impl Pipeline {
    pub const DEFAULT_MAX_SWEEPS: u32 = 16;

    pub fn empty() -> Pipeline {
        Pipeline {
            passes: Vec::new(),
            max_sweeps: Self::DEFAULT_MAX_SWEEPS,
        }
    }

    /// Build the preset pipeline for a level.
    pub fn for_level(level: OptLevel) -> Pipeline {
        let mut p = Pipeline::empty();
        match level {
            OptLevel::O0 => {}
            OptLevel::O1 => {
                p.push(Box::new(normalize::Normalize));
                p.push(Box::new(drain_loop::DrainLoop));
                p.push(Box::new(dead_code::DeadCode));
            }
            OptLevel::O2 => {
                p.push(Box::new(normalize::Normalize));
                p.push(Box::new(drain_loop::DrainLoop));
                p.push(Box::new(scan::ScanLoop));
                p.push(Box::new(const_fold::ConstFold));
                p.push(Box::new(dead_code::DeadCode));
            }
            OptLevel::O3 => {
                p.push(Box::new(normalize::Normalize));
                p.push(Box::new(drain_loop::DrainLoop));
                p.push(Box::new(scan::ScanLoop));
                p.push(Box::new(const_fold::ConstFold));
                p.push(Box::new(unroll::LoopUnroll::default()));
                p.push(Box::new(partial_eval::PartialEval::default()));
                p.push(Box::new(dead_code::DeadCode));
            }
        }
        p
    }

    pub fn push(&mut self, pass: Box<dyn Pass>) -> &mut Self {
        self.passes.push(pass);
        self
    }

    /// Remove every pass with this name, returning how many were removed.
    pub fn disable(&mut self, name: &str) -> usize {
        let before = self.passes.len();
        self.passes.retain(|p| p.name() != name);
        before - self.passes.len()
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.passes.iter().map(|p| p.name()).collect()
    }

    pub fn passes(&self) -> &[Box<dyn Pass>] {
        &self.passes
    }

    pub fn is_empty(&self) -> bool {
        self.passes.is_empty()
    }

    /// Run every pass in order, repeating until a full sweep reports no
    /// change or `max_sweeps` is reached.
    pub fn run(&self, program: &mut Program, ctx: &Ctx<'_>) -> Stats {
        debug_validate(program, ctx.dialect, "lowering");
        let mut stats = Stats {
            ops_before: program.op_count(),
            passes: self
                .passes
                .iter()
                .map(|p| PassStat {
                    name: p.name(),
                    runs: 0,
                    changes: 0,
                })
                .collect(),
            ..Stats::default()
        };
        while stats.sweeps < self.max_sweeps {
            stats.sweeps += 1;
            let mut changed = false;
            for (pass, stat) in self.passes.iter().zip(&mut stats.passes) {
                stat.runs += 1;
                if pass.run(program, ctx) {
                    stat.changes += 1;
                    changed = true;
                }
                debug_validate(program, ctx.dialect, pass.name());
            }
            if !changed {
                break;
            }
        }
        stats.ops_after = program.op_count();
        stats
    }
}

/// In debug builds, panic if `program` breaks an IR invariant. `after`
/// names the pass that just ran, for the message.
fn debug_validate(program: &Program, dialect: &Dialect, after: &str) {
    if cfg!(debug_assertions)
        && let Err(violation) = program.validate(dialect)
    {
        panic!("IR invariant broken after {after}: {violation}");
    }
}

impl core::fmt::Debug for Pipeline {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Pipeline")
            .field("passes", &self.names())
            .field("max_sweeps", &self.max_sweeps)
            .finish()
    }
}

/// What the pipeline did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub sweeps: u32,
    pub ops_before: usize,
    pub ops_after: usize,
    pub passes: Vec<PassStat>,
}

impl Stats {
    pub fn reached_fixed_point(&self, pipeline: &Pipeline) -> bool {
        self.sweeps < pipeline.max_sweeps
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassStat {
    pub name: &'static str,
    /// How many times the pass ran.
    pub runs: u32,
    /// How many times the pass reported a change.
    pub changes: u32,
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::ir::lower;
    use crate::parser;

    /// Reports a change for the first `n` runs, then never again.
    struct Countdown(Cell<u32>);

    impl Countdown {
        fn new(n: u32) -> Countdown {
            Countdown(Cell::new(n))
        }
    }

    impl Pass for Countdown {
        fn name(&self) -> &'static str {
            "Countdown"
        }

        fn run(&self, _program: &mut Program, _ctx: &Ctx<'_>) -> Changed {
            let left = self.0.get();
            self.0.set(left.saturating_sub(1));
            left > 0
        }
    }

    fn program() -> Program {
        lower::lower(&parser::parse("+++[>++++<-]>.").expect("parses"))
    }

    #[test]
    fn runs_to_a_fixed_point() {
        let mut pipeline = Pipeline::empty();
        pipeline.push(Box::new(Countdown::new(2)));
        let stats = pipeline.run(&mut program(), &Ctx::new(&Dialect::default()));

        // Two sweeps that change something, then the sweep that sees no
        // change.
        assert_eq!(stats.sweeps, 3);
        assert_eq!(stats.passes.len(), 1);
        assert_eq!(stats.passes[0].runs, 3);
        assert_eq!(stats.passes[0].changes, 2);
        assert!(stats.reached_fixed_point(&pipeline));
    }

    #[test]
    fn max_sweeps_bounds_a_spinning_pass() {
        let mut pipeline = Pipeline::empty();
        pipeline.max_sweeps = 4;
        pipeline.push(Box::new(Countdown::new(u32::MAX)));
        let stats = pipeline.run(&mut program(), &Ctx::new(&Dialect::default()));

        assert_eq!(stats.sweeps, 4);
        assert!(!stats.reached_fixed_point(&pipeline));
    }

    #[test]
    fn stats_record_op_counts() {
        let mut p = program();
        let expected = p.op_count();
        let stats = Pipeline::empty().run(&mut p, &Ctx::new(&Dialect::default()));
        assert_eq!(stats.ops_before, expected);
        assert_eq!(stats.ops_after, expected);
        assert_eq!(stats.sweeps, 1);
    }

    /// Replaces the program with a scan whose stride is 0.
    struct Vandal;

    impl Pass for Vandal {
        fn name(&self) -> &'static str {
            "Vandal"
        }

        fn run(&self, program: &mut Program, _ctx: &Ctx<'_>) -> Changed {
            use crate::error::Span;
            use crate::ir::{Block, Node, NodeKind, Scan};
            let scan = Node::new(NodeKind::Scan(Scan { stride: 0 }), Span::SYNTHETIC);
            program.body = Block::from_nodes(vec![scan]);
            true
        }
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "IR invariant broken after Vandal")]
    fn a_pass_that_breaks_an_invariant_panics_in_debug_builds() {
        let mut pipeline = Pipeline::empty();
        pipeline.push(Box::new(Vandal));
        pipeline.run(&mut program(), &Ctx::new(&Dialect::default()));
    }

    #[test]
    fn disable_removes_by_name() {
        let mut pipeline = Pipeline::for_level(OptLevel::O1);
        assert_eq!(pipeline.disable("DrainLoop"), 1);
        assert_eq!(pipeline.names(), vec!["Normalize", "DeadCode"]);
        assert_eq!(pipeline.disable("NoSuchPass"), 0);
    }
}
