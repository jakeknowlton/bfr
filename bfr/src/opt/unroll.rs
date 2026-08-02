//! Replaces a loop whose trip count is statically known by that many copies of its body, bounded by size limits.
//!
//! Examples
//!
//! `[p] = 2; while [p] { write([p+1]); [p] -= 1 }` becomes `[p] = 2; write([p+1]); [p] -= 1; write([p+1]); [p] -= 1`

use crate::ir::Program;
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
        todo!("LoopUnroll::run")
    }
}
