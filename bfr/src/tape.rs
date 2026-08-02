//! The tape.

use crate::config::Dialect;

/// Stored as raw bytes so one representation serves all three cell widths.
#[derive(Debug)]
pub struct Tape {
    bytes: Vec<u8>,
    dialect: Dialect,
}

impl Tape {
    pub fn new(dialect: &Dialect) -> Self {
        Tape {
            bytes: vec![0; dialect.tape_cells * dialect.cell_width.bytes()],
            dialect: *dialect,
        }
    }

    pub fn len_cells(&self) -> usize {
        self.dialect.tape_cells
    }

    /// Read one cell, honoring the configured width. `None` out of range.
    pub fn get(&self, index: usize) -> Option<crate::ir::Cell> {
        todo!("Tape::get")
    }

    pub fn set(&mut self, index: usize, value: crate::ir::Cell) {
        todo!("Tape::set")
    }

    /// Zero every cell without reallocating.
    pub fn reset(&mut self) {
        self.bytes.fill(0);
    }
}
