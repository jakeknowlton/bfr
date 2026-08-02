//! Rewrites loops that only move the pointer into a scan.
//!
//! Examples
//!
//! `[>]` becomes `scan p += 1`
//! `[>>>]` becomes `scan p += 3`

use crate::ir::Program;
use crate::opt::{Changed, Ctx, Pass};

pub struct ScanLoop;

impl Pass for ScanLoop {
    fn name(&self) -> &'static str {
        "ScanLoop"
    }

    fn run(&self, program: &mut Program, ctx: &Ctx<'_>) -> Changed {
        todo!("ScanLoop::run -- try_replace_loops over LoopShape::Scan")
    }
}
