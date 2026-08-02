//! Deletes a write that nothing observes before a later write to the same cell overwrites it,
//! looking across unrelated intervening effects.
//!
//! Examples
//!
//! `[p] += 3; [p+1] += 1; [p] = 0` becomes `[p] = 0`

use crate::ir::Program;
use crate::opt::{Changed, Ctx, Pass};

pub struct DeadStoreElim;

impl Pass for DeadStoreElim {
    fn name(&self) -> &'static str {
        "DeadStoreElim"
    }

    fn run(&self, program: &mut Program, ctx: &Ctx<'_>) -> Changed {
        todo!("DeadStoreElim::run")
    }
}
