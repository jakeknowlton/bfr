//! What the optimizer knows about the tape at one point in a block. Shared
//! by every pass that reasons about cell values.

use std::collections::HashMap;

use crate::config::Dialect;
use crate::ir::{BlockSite, Cell, EffKind, Loop, Run, arith};

/// What is known about one cell's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Known {
    Value(Cell),
    Unknown,
}

/// What the tape holds at one point in a block. `cells` is keyed by offset
/// from the pointer at block entry, and `origin` is the pointer's current
/// offset from there, so a shift is one addition. A cell gets an entry once
/// something has accessed it. Every other cell holds `rest`.
///
pub struct State {
    cells: HashMap<isize, Known>,
    rest: Known,
    origin: isize,
}

impl State {
    pub fn on_entry(site: BlockSite) -> State {
        let rest = match site {
            BlockSite::Program => Known::Value(0),
            BlockSite::LoopBody => Known::Unknown,
        };
        // Whatever entered the block read the cell under the pointer.
        State {
            cells: HashMap::from([(0, rest)]),
            rest,
            origin: 0,
        }
    }

    pub fn get(&self, at: isize) -> Known {
        self.cells
            .get(&(self.origin + at))
            .copied()
            .unwrap_or(self.rest)
    }

    pub fn set(&mut self, at: isize, value: Known) {
        self.cells.insert(self.origin + at, value);
    }

    /// Record an access that leaves the cell as it was.
    pub fn touch(&mut self, at: isize) {
        let value = self.get(at);
        self.cells.entry(self.origin + at).or_insert(value);
    }

    /// Whether an access has proven the cell is on the tape.
    pub fn accessed(&self, at: isize) -> bool {
        self.cells.contains_key(&(self.origin + at))
    }

    pub fn forget_all(&mut self) {
        self.cells.clear();
        self.rest = Known::Unknown;
    }

    /// Move the pointer.
    pub fn shift(&mut self, by: isize) {
        self.origin += by;
    }

    /// The pointer's offset from where it was at block entry.
    pub fn origin(&self) -> isize {
        self.origin
    }

    /// Every cell with a known value, as `(offset from block entry, value)`.
    pub fn known_cells(&self) -> impl Iterator<Item = (isize, Cell)> + '_ {
        self.cells.iter().filter_map(|(&key, &known)| match known {
            Known::Value(value) => Some((key, value)),
            Known::Unknown => None,
        })
    }

    /// Advance past a run without rewriting it.
    pub fn advance(&mut self, run: &Run, dialect: &Dialect) {
        for eff in &run.effects {
            self.step(&eff.kind, dialect);
        }
        self.shift(run.shift);
    }

    /// Advance past a loop that stays in the program. A known zero control
    /// cell means the loop did nothing but read it. Otherwise every cell the
    /// loop can write is unknown afterwards, and its control cell is zero.
    pub fn pass_loop(&mut self, l: &Loop) {
        if self.get(0) == Known::Value(0) {
            self.touch(0);
            return;
        }
        match l.footprint() {
            Some(footprint) => {
                for at in footprint.writes {
                    self.set(at, Known::Unknown);
                }
            }
            None => self.forget_all(),
        }
        self.set(0, Known::Value(0));
    }

    /// Advance past a scan that stays in the program. A known zero control
    /// cell means the scan did nothing but read it. Otherwise the pointer
    /// could be anywhere afterwards, so only the cell it stopped on is known.
    pub fn pass_scan(&mut self) {
        if self.get(0) == Known::Value(0) {
            self.touch(0);
            return;
        }
        self.forget_all();
        self.set(0, Known::Value(0));
    }

    /// Apply one effect to the state. When its inputs are known, return
    /// the store or plain add it amounts to.
    pub fn step(&mut self, kind: &EffKind, dialect: &Dialect) -> Option<EffKind> {
        match *kind {
            EffKind::Add { at, delta } => match self.get(at) {
                Known::Value(v) => {
                    let value = arith::apply_delta(v, delta, dialect);
                    self.set(at, Known::Value(value));
                    Some(EffKind::Store { at, value })
                }
                Known::Unknown => {
                    self.set(at, Known::Unknown);
                    None
                }
            },
            EffKind::Store { at, value } => {
                self.set(at, Known::Value(value));
                None
            }
            EffKind::AddScaled { at, from, factor } => {
                self.touch(from);
                match (self.get(at), self.get(from)) {
                    (Known::Value(a), Known::Value(f)) => {
                        let delta = arith::scaled_delta(f, factor, dialect);
                        let value = arith::apply_delta(a, delta, dialect);
                        self.set(at, Known::Value(value));
                        Some(EffKind::Store { at, value })
                    }
                    (Known::Unknown, Known::Value(f)) => {
                        self.set(at, Known::Unknown);
                        let delta = arith::scaled_delta(f, factor, dialect);
                        Some(EffKind::Add { at, delta })
                    }
                    (_, Known::Unknown) => {
                        self.set(at, Known::Unknown);
                        None
                    }
                }
            }
            EffKind::Read { at } => {
                self.set(at, Known::Unknown);
                None
            }
            EffKind::Write { at } => {
                self.touch(at);
                None
            }
        }
    }
}
