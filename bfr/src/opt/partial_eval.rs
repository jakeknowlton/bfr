//! Executes the whole program at compile time until it hits something undecidable
//! (input, an unknowable trip count, its fuel or output budget),
//! then emits only the residue.
//!
//! Examples
//!
//! `[p] += 3; while [p] { [p+1] += 2; [p] -= 1 }; [p] = read()` becomes `[p+1] = 6; [p] = read()`.
//! `hello.b` becomes `[p] = 72; write([p]); [p] = 101; write([p]); ...`.

use crate::ir::Program;
use crate::opt::{Changed, Ctx, Pass};

pub struct PartialEval {
    /// Maximum abstract steps to execute at compile time.
    pub fuel: u64,
    /// Maximum bytes of statically-known output to materialize.
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
