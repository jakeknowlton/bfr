//! Combines adjacent effects and merges adjacent runs,
//! using no knowledge beyond what's textually next to each other.
//!
//! Exmples
//!
//! `+++` becomes `[p] += 3`.
//! `>>>>>` becomes `p += 5`.

use crate::ir::Program;
use crate::opt::{Changed, Ctx, Pass};

pub struct Normalize;

impl Pass for Normalize {
    fn name(&self) -> &'static str {
        "Normalize"
    }

    fn run(&self, program: &mut Program, ctx: &Ctx<'_>) -> Changed {
        todo!("Normalize::run -- for_each_block_mut + Block::normalize")
    }
}
