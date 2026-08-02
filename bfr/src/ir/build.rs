//! Incremental IR construction with cursor tracking.

use crate::error::Span;
use crate::ir::{EffKind, Program};

pub struct Builder {
    _private: (),
}

impl Builder {
    pub fn new() -> Builder {
        todo!("Builder::new")
    }

    /// `>` / `<`: move the cursor. The span merges into the current run.
    pub fn bump(&mut self, delta: isize, span: Span) {
        todo!("Builder::bump")
    }

    /// Push one effect at cursor-relative offsets.
    pub fn push(&mut self, kind: EffKind, span: Span) {
        todo!("Builder::push")
    }

    /// Seal the current run and emit a [`crate::ir::Scan`] node.
    pub fn scan(&mut self, stride: isize, span: Span) {
        todo!("Builder::scan")
    }

    /// `[`: seal the current run and open a loop body.
    pub fn begin_loop(&mut self, span: Span) {
        todo!("Builder::begin_loop")
    }

    /// `]`: close the innermost loop.
    pub fn end_loop(&mut self, span: Span) {
        todo!("Builder::end_loop")
    }

    /// Seal the final run and hand back the program.
    ///
    /// # Panics
    ///
    /// Panics if a loop is still open.
    pub fn finish(self) -> Program {
        todo!("Builder::finish")
    }
}

impl Default for Builder {
    fn default() -> Self {
        Builder::new()
    }
}
