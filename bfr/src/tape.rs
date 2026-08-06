//! The tape.

use crate::config::{CellWidth, Dialect};
use crate::ir;

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
    pub fn get(&self, index: usize) -> Option<ir::Cell> {
        let width = self.dialect.cell_width.bytes();
        let bytes = self.bytes.get(index.checked_mul(width)?..)?.get(..width)?;
        Some(match self.dialect.cell_width {
            CellWidth::U8 => bytes[0].into(),
            CellWidth::U16 => u16::from_ne_bytes(bytes.try_into().unwrap()).into(),
            CellWidth::U32 => u32::from_ne_bytes(bytes.try_into().unwrap()),
        })
    }

    /// Write one cell, truncating `value` to the configured width.
    ///
    /// # Panics
    ///
    /// Panics if `index` is out of range.
    pub fn set(&mut self, index: usize, value: ir::Cell) {
        let width = self.dialect.cell_width.bytes();
        let start = index * width;
        let bytes = &mut self.bytes[start..start + width];
        match self.dialect.cell_width {
            CellWidth::U8 => bytes[0] = value as u8,
            CellWidth::U16 => bytes.copy_from_slice(&(value as u16).to_ne_bytes()),
            CellWidth::U32 => bytes.copy_from_slice(&value.to_ne_bytes()),
        }
    }

    /// Zero every cell without reallocating.
    pub fn reset(&mut self) {
        self.bytes.fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tape(cell_width: CellWidth) -> Tape {
        Tape::new(&Dialect {
            cell_width,
            tape_cells: 4,
            ..Dialect::default()
        })
    }

    #[test]
    fn round_trips_at_every_width() {
        for width in [CellWidth::U8, CellWidth::U16, CellWidth::U32] {
            let mut t = tape(width);
            t.set(2, 0x0102_0304);
            assert_eq!(t.get(2), Some(0x0102_0304 & width.mask()));
            // Neighbors are untouched.
            assert_eq!(t.get(1), Some(0));
            assert_eq!(t.get(3), Some(0));
        }
    }

    #[test]
    fn get_out_of_range_is_none() {
        assert_eq!(tape(CellWidth::U8).get(4), None);
        assert_eq!(tape(CellWidth::U32).get(usize::MAX), None);
    }

    #[test]
    #[should_panic]
    fn set_out_of_range_panics() {
        tape(CellWidth::U8).set(4, 1);
    }
}
