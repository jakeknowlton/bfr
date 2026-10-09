//! Runs the program at compile time, tracking what every cell holds. An
//! effect whose inputs are known is applied to that tracked state. An effect
//! that depends on an unknown value, such as the byte a read produces, is
//! kept and leaves its cell unknown. Evaluation stops only when it can no
//! longer continue, such as at a loop whose control cell is unknown, or when
//! the fuel or output budget runs out.
//!
//! Examples
//!
//! `[p] += 3; while [p] { [p+1] += 2; [p] -= 1 }; [p] = read()` becomes `[p+1] = 6; [p] = read()`.
//! `hello.b` becomes `[p] = 72; write([p]); [p] = 101; write([p]); ...`.

use crate::ir::Program;
use crate::opt::{Changed, Ctx, Pass};

pub struct PartialEval {
    /// Maximum steps to execute at compile time.
    pub fuel: u64,
    /// Maximum bytes of compile-time output to keep.
    pub max_output: usize,
}

impl Default for PartialEval {
    fn default() -> Self {
        PartialEval {
            fuel: 10_000_000,
            max_output: 1 << 16,
        }
    }
}

impl Pass for PartialEval {
    fn name(&self) -> &'static str {
        "PartialEval"
    }

    fn run(&self, program: &mut Program, ctx: &Ctx<'_>) -> Changed {
        todo!("PartialEval::run")
    }
}
