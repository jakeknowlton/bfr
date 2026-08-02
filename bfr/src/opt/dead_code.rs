//! Deletes loops that provably never execute, through three cheap
//! patterns where the control cell must be zero:
//! at program start, right after another loop, or right after `[p] = 0`.
//!
//! Examples
//!
//! `while [p] { [p] -= 1 }; [p] += 1` at program start becomes `[p] += 1`.
//! `while [p] { [p] -= 1 }; while [p] { [p+1] += 1 }` becomes `while [p] { [p] -= 1 }` (a loop only exits at zero).
//! `[p] = 0; while [p] { [p+1] += 1 }` becomes `[p] = 0`.

use crate::ir::Program;
use crate::opt::{Changed, Ctx, Pass};

pub struct DeadCode;

impl Pass for DeadCode {
    fn name(&self) -> &'static str {
        "DeadCode"
    }

    fn run(&self, program: &mut Program, ctx: &Ctx<'_>) -> Changed {
        todo!("DeadCode::run")
    }
}
