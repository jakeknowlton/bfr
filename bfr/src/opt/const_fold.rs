//! Tracks which cells hold known values (seeded by the zeroed tape),
//! turning arithmetic on known cells into stores and settling loops whose
//! control value it knows: a known zero deletes the loop; a known nonzero
//! replaces it with its single trip's effects, when the body is a
//! straight-line run that zeroes its own control cell.
//!
//! Examples:
//!
//! `[p] += 3` at program start becomes `[p] = 3`.
//! `[p] = 3; while [p] { [p+1] += [p] * 4; [p] = 0 }` becomes `[p] = 3; [p+1] = 12; [p] = 0`.

use crate::ir::Program;
use crate::opt::{Changed, Ctx, Pass};

pub struct ConstFold;

impl Pass for ConstFold {
    fn name(&self) -> &'static str {
        "ConstFold"
    }

    fn run(&self, program: &mut Program, ctx: &Ctx<'_>) -> Changed {
        todo!("ConstFold::run")
    }
}

/// Abstract value for one cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Known {
    Value(crate::ir::Cell),
    Unknown,
}
