//! Rewrites loops that step their control cell by a terminating constant into direct arithmetic,
//! turning O(cell value) iteration into O(1).
//!
//! Examples
//!
//! `[-]` becomes `[p] = 0`.
//! `[->+<]` becomes `[p+1] += [p]; [p] = 0`.

use crate::ir::Program;
use crate::opt::{Changed, Ctx, Pass};

pub struct DrainLoop;

impl Pass for DrainLoop {
    fn name(&self) -> &'static str {
        "DrainLoop"
    }

    fn run(&self, program: &mut Program, ctx: &Ctx<'_>) -> Changed {
        todo!("DrainLoop::run -- try_replace_loops over Loop::shape")
    }
}
