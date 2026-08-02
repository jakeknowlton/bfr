//! Compilation configuration: dialect, pipeline, and runtime limits.

use crate::opt::Pipeline;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CellWidth {
    #[default]
    U8,
    U16,
    U32,
}

impl CellWidth {
    pub const fn bytes(self) -> usize {
        match self {
            CellWidth::U8 => 1,
            CellWidth::U16 => 2,
            CellWidth::U32 => 4,
        }
    }

    /// Mask that truncates a [`crate::ir::Cell`] to this width.
    pub const fn mask(self) -> u32 {
        match self {
            CellWidth::U8 => 0xff,
            CellWidth::U16 => 0xffff,
            CellWidth::U32 => 0xffff_ffff,
        }
    }
}

/// What `,` stores when the host has no more input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EofBehavior {
    /// Store 0.
    #[default]
    Zero,
    /// Leave the cell as it was.
    Unchanged,
    /// Store the all-ones value for the cell width (255 for `U8`).
    MinusOne,
}

/// Semantics of the brainfuck being compiled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dialect {
    pub cell_width: CellWidth,
    pub eof: EofBehavior,
    pub tape_cells: usize,
    /// Cell index the pointer starts at.
    pub origin: usize,
}

impl Default for Dialect {
    fn default() -> Self {
        Dialect {
            cell_width: CellWidth::U8,
            eof: EofBehavior::Zero,
            tape_cells: 1 << 21,
            origin: 0,
        }
    }
}

/// Optimization preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum OptLevel {
    /// Mechanical lowering only.
    O0,
    #[default]
    O1,
    O2,
    /// Adds unrolling and partial evaluation.
    O3,
}

pub struct Config {
    pub dialect: Dialect,
    /// The passes to run, in order.
    pub pipeline: Pipeline,
    pub fuel: Option<u64>,
}

impl Config {
    /// Default dialect, default limits, pipeline built from `level`.
    pub fn new(level: OptLevel) -> Self {
        Config {
            dialect: Dialect::default(),
            pipeline: Pipeline::for_level(level),
            fuel: None,
        }
    }

    pub fn with_dialect(mut self, dialect: Dialect) -> Self {
        self.dialect = dialect;
        self
    }

    pub fn with_fuel(mut self, fuel: u64) -> Self {
        self.fuel = Some(fuel);
        self
    }
}

impl Default for Config {
    fn default() -> Self {
        Config::new(OptLevel::default())
    }
}

impl core::fmt::Debug for Config {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Config")
            .field("dialect", &self.dialect)
            .field("pipeline", &self.pipeline)
            .field("fuel", &self.fuel)
            .finish()
    }
}
