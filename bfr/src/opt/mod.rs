//! The optimization pipeline.

pub mod const_fold;
pub mod dead_code;
pub mod dead_store;
pub mod drain_loop;
pub mod normalize;
pub mod partial_eval;
pub mod scan;
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
    /// Safety valve to avoid optimizations taking too long.
    pub max_iterations: u32,
}

impl Pipeline {
    pub const DEFAULT_MAX_ITERATIONS: u32 = 16;

    pub fn empty() -> Pipeline {
        Pipeline {
            passes: Vec::new(),
            max_iterations: Self::DEFAULT_MAX_ITERATIONS,
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
                p.push(Box::new(dead_store::DeadStoreElim));
                p.push(Box::new(dead_code::DeadCode));
            }
            OptLevel::O3 => {
                p.push(Box::new(normalize::Normalize));
                p.push(Box::new(drain_loop::DrainLoop));
                p.push(Box::new(scan::ScanLoop));
                p.push(Box::new(const_fold::ConstFold));
                p.push(Box::new(unroll::LoopUnroll::default()));
                p.push(Box::new(partial_eval::PartialEval::default()));
                p.push(Box::new(dead_store::DeadStoreElim));
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
        todo!("Pipeline::disable")
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

    /// Repeatedly un every pass in order until a full sweep reports no change or `max_iterations` is reached.
    pub fn run(&self, program: &mut Program, ctx: &Ctx<'_>) -> Stats {
        // TODO: Assert [`Program::validate`] in debug builds
        todo!("Pipeline::run")
    }
}

impl core::fmt::Debug for Pipeline {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Pipeline")
            .field("passes", &self.names())
            .field("max_iterations", &self.max_iterations)
            .finish()
    }
}

/// What the pipeline did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub iterations: u32,
    pub ops_before: usize,
    pub ops_after: usize,
    pub passes: Vec<PassStat>,
}

impl Stats {
    pub fn reached_fixed_point(&self, pipeline: &Pipeline) -> bool {
        self.iterations < pipeline.max_iterations
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassStat {
    pub name: &'static str,
    /// Invocations.
    pub runs: u32,
    /// How many of runs reported a change.
    pub changes: u32,
}
