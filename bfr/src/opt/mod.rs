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
use const_fold::ConstFold;
use dead_code::DeadCode;
use drain_loop::DrainLoop;
use normalize::Normalize;
use partial_eval::PartialEval;
use scan::ScanLoop;
use unroll::LoopUnroll;

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

/// A constructor for a built-in pass with its default settings.
type Constructor = fn() -> Box<dyn Pass>;

const NORMALIZE: Constructor = || Box::new(Normalize);
const DRAIN_LOOP: Constructor = || Box::new(DrainLoop);
const SCAN_LOOP: Constructor = || Box::new(ScanLoop);
const CONST_FOLD: Constructor = || Box::new(ConstFold);
const LOOP_UNROLL: Constructor = || Box::new(LoopUnroll::default());
const PARTIAL_EVAL: Constructor = || Box::new(PartialEval::default());
const DEAD_CODE: Constructor = || Box::new(DeadCode);

/// Every built-in pass.
const REGISTRY: [Constructor; 7] = [
    NORMALIZE,
    DRAIN_LOOP,
    SCAN_LOOP,
    CONST_FOLD,
    LOOP_UNROLL,
    PARTIAL_EVAL,
    DEAD_CODE,
];

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

    /// The names of every built-in pass, as [`Pass::name`] reports them.
    pub fn pass_names() -> Vec<&'static str> {
        REGISTRY.iter().map(|build| build().name()).collect()
    }

    /// Build a pipeline from pass names, in order.
    ///
    /// # Errors
    ///
    /// Returns a message naming the first unknown pass and listing the
    /// known ones.
    pub fn named<S: AsRef<str>>(names: &[S]) -> Result<Pipeline, String> {
        let mut p = Pipeline::empty();
        for name in names {
            let name = name.as_ref();
            let Some(pass) = REGISTRY.iter().map(|build| build()).find(|pass| pass.name() == name)
            else {
                return Err(format!(
                    "no pass named {name:?}, the passes are {}",
                    Self::pass_names().join(", ")
                ));
            };
            p.push(pass);
        }
        Ok(p)
    }

    /// Build the preset pipeline for a level.
    pub fn for_level(level: OptLevel) -> Pipeline {
        let passes: &[Constructor] = match level {
            OptLevel::O0 => &[],
            OptLevel::O1 => &[NORMALIZE, DRAIN_LOOP, DEAD_CODE],
            OptLevel::O2 => &[NORMALIZE, DRAIN_LOOP, SCAN_LOOP, CONST_FOLD, DEAD_CODE],
            OptLevel::O3 => &[
                NORMALIZE,
                DRAIN_LOOP,
                SCAN_LOOP,
                CONST_FOLD,
                LOOP_UNROLL,
                PARTIAL_EVAL,
                DEAD_CODE,
            ],
        };
        let mut p = Pipeline::empty();
        for build in passes {
            p.push(build());
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

/// A short report: the sweep and op counts, then one line per pass.
impl core::fmt::Display for Stats {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        writeln!(
            f,
            "pipeline: {} {}, {} -> {} ops",
            self.sweeps,
            if self.sweeps == 1 { "sweep" } else { "sweeps" },
            self.ops_before,
            self.ops_after
        )?;
        let width = self.passes.iter().map(|p| p.name.len()).max().unwrap_or(0);
        for pass in &self.passes {
            writeln!(
                f,
                "  {:width$}  runs {:>2}  changes {:>2}",
                pass.name, pass.runs, pass.changes
            )?;
        }
        Ok(())
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
    fn stats_display_lists_every_pass() {
        let mut pipeline = Pipeline::empty();
        pipeline.push(Box::new(Countdown::new(1)));
        let stats = pipeline.run(&mut program(), &Ctx::new(&Dialect::default()));
        assert_eq!(
            stats.to_string(),
            "pipeline: 2 sweeps, 10 -> 10 ops\n  Countdown  runs  2  changes  1\n"
        );
        let stats = Pipeline::empty().run(&mut program(), &Ctx::new(&Dialect::default()));
        assert_eq!(stats.to_string(), "pipeline: 1 sweep, 10 -> 10 ops\n");
    }

    #[test]
    fn named_builds_passes_in_order() {
        let pipeline = Pipeline::named(&["DeadCode", "Normalize"]).expect("known names");
        assert_eq!(pipeline.names(), vec!["DeadCode", "Normalize"]);
        assert!(Pipeline::named::<&str>(&[]).expect("empty").is_empty());
    }

    #[test]
    fn named_rejects_an_unknown_pass() {
        let error = Pipeline::named(&["Normalize", "Shrink"]).expect_err("unknown");
        assert_eq!(
            error,
            "no pass named \"Shrink\", the passes are Normalize, DrainLoop, ScanLoop, \
             ConstFold, LoopUnroll, PartialEval, DeadCode"
        );
    }

    #[test]
    fn every_built_in_pass_reports_its_registered_name() {
        let names = Pipeline::pass_names();
        let pipeline = Pipeline::named(&names).expect("all known");
        assert_eq!(pipeline.names(), names);
    }

    #[test]
    fn disable_removes_by_name() {
        let mut pipeline = Pipeline::for_level(OptLevel::O1);
        assert_eq!(pipeline.disable("DrainLoop"), 1);
        assert_eq!(pipeline.names(), vec!["Normalize", "DeadCode"]);
        assert_eq!(pipeline.disable("NoSuchPass"), 0);
    }
}
